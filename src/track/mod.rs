//! 轨迹层：生成器 / OBS 组装 / 官方卡路里。

pub mod calorie;
pub mod altitude;
pub mod geom;
pub mod generator;
pub mod model;
pub mod postfix;
pub mod roads;
pub mod wire;

#[cfg(test)]
mod tests {
    use super::geom::{MET_PER_DEG_LAT, MET_PER_DEG_LNG};
    use super::generator::build;
    
    use super::wire::*;

    fn sample_points() -> Vec<(f64, f64)> {
        // 某校园 5 个打卡点（BD 系）
        vec![
            (38.901678, 121.540241),
            (38.902564, 121.541233),
            (38.900921, 121.542310),
            (38.899823, 121.541010),
            (38.900455, 121.539512),
        ]
    }

    /// 轨迹生成抽样断言：距离精确、采样间隔分布、哨兵/断崖/位移语义。
    #[test]
    fn test_generator_distribution() {
        let pts = sample_points();
        let start = 1_788_958_186_123i64;
        let track = build(3300.0, 1220, 42, (38.9, 121.54), start, &pts);
        assert!(track.locations.iter().all(|point| point.ptype != -1), "campus track must not contain invalid drift points");
        // 总距离精确等于目标（±0.5m 舍入容差）
        assert!((track.totalDistance - 3300.0).abs() < 0.5, "dist={}", track.totalDistance);
        assert_eq!(track.totalTime, 1220);
        // 点数合理（主 5s 采样）
        let n = track.locations.len();
        assert!((200..320).contains(&n), "n={n}");
        // 哨兵：索引0 type∈{0,7}/totalTime=0/state=1；索引1 type=5 全零；末点 type=6
        assert!([0, 7].contains(&track.locations[0].ptype));
        assert_eq!(track.locations[0].totalTime, 0);
        assert_eq!(track.locations[0].state, 1);
        assert_eq!(track.locations[1].ptype, 5);
        assert_eq!(track.locations[1].totalDis, 0.0);
        assert_eq!(track.locations[1].steps, 0);
        assert_eq!(track.locations.last().unwrap().ptype, 6);
        // 累计距离单调不减、末点 ≈ 总距离
        let mut prev = 0.0;
        for p in &track.locations {
            assert!(p.totalDis >= prev - 1e-6, "totalDis 回退");
            prev = p.totalDis;
        }
        // 距离只由正常点承担：终点哨兵(type=6)携带全程累计距离
        assert!(
            (track.locations.last().unwrap().totalDis - 3300.0).abs() < 2.0,
            "末点={}",
            track.locations.last().unwrap().totalDis
        );
        // 采样间隔：5s 占比 ≥ 60%
        let mut fives = 0;
        let mut total = 0;
        for w in track.locations.windows(2) {
            let dt = w[1].totalTime - w[0].totalTime;
            if dt > 0 {
                total += 1;
                if dt == 5 {
                    fives += 1;
                }
            }
        }
        assert!(fives as f64 / total as f64 > 0.6, "5s 占比不足");
        // 10s 窗非空、结构合法
        assert!(!track.speedPerTenSec.is_empty());
        assert_eq!(track.speedPerTenSec.len(), track.stepsPerTenSec.len());
        // 首点 lat/lng 占位 -1.0，coorType gcj02
        assert_eq!(track.locations[0].lat, -1.0);
        assert_eq!(track.locations[0].coorType, "gcj02");
        // 步数为正、步频在合理范围
        assert!(track.totalSteps > 500, "steps={}", track.totalSteps);
    }

    /// 打卡点吸附：轨迹必过点位（<40m 落位）。
    #[test]
    fn test_point_snapping() {
        let pts = sample_points();
        let track = build(2200.0, 900, 7, (38.9, 121.54), 1_788_958_186_123, &pts);
        for pl in &pts {
            let min_m = track
                .locations
                .iter()
                .map(|p| {
                    (((p.gLat - pl.0) * MET_PER_DEG_LAT).powi(2)
                        + ((p.gLng - pl.1) * MET_PER_DEG_LNG).powi(2))
                    .sqrt()
                })
                .fold(f64::INFINITY, f64::min);
assert!(min_m < 1.0, "点位吸附失败: {min_m}m");
        }
    }

    /// 折线环底环：矩形道路环上生成，轨迹应贴着环走且必过全部打卡点。
    #[test]
    fn test_build_with_road_ring() {
        // 以打卡点为顶点的矩形闭合环（模拟路网输出的道路环，BD 系）
        let pts = sample_points();
        let mut ring: Vec<(f64, f64)> = Vec::new();
        // 极角排序后按顺序连线（与道路环语义一致）
        let order = super::roads::angular_order(&pts);
        for &i in &order {
            ring.push(pts[i]);
        }
        ring.push(pts[order[0]]);
        // 中间加密几个点模拟道路折线
        let mut dense_ring = Vec::new();
        for w in ring.windows(2) {
            for j in 0..8 {
                let t = j as f64 / 8.0;
                dense_ring.push((w[0].0 + (w[1].0 - w[0].0) * t, w[1].1 * t + w[0].1 * (1.0 - t)));
            }
        }
        let track = super::generator::build_with_ring(
            2600.0,
            960,
            11,
            (38.9, 121.54),
            1_788_958_186_123,
            &pts,
            Some(&dense_ring),
        );
        assert!((track.totalDistance - 2600.0).abs() < 0.5, "dist={}", track.totalDistance);
        // 所有打卡点仍被精确命中
        for pl in &pts {
            let min_m = track
                .locations
                .iter()
                .map(|p| {
                    (((p.gLat - pl.0) * MET_PER_DEG_LAT).powi(2)
                        + ((p.gLng - pl.1) * MET_PER_DEG_LNG).powi(2))
                    .sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            assert!(min_m < 1.0, "道路环下点位吸附失败: {min_m}m");
        }
    }

    /// 折线环弧长表：闭合环总弧长 = 周长，插值点严格落在线段上。
    #[test]
    fn test_make_polyline_ring_arcs() {
        use super::geom::{make_polyline_ring, ring_point_at};
        // 100m × 200m 矩形
        let lat0 = 38.9;
        let lng0 = 121.54;
        let ring = vec![
            (lat0, lng0),
            (lat0, lng0 + 100.0 / MET_PER_DEG_LNG),
            (lat0 + 200.0 / MET_PER_DEG_LAT, lng0 + 100.0 / MET_PER_DEG_LNG),
            (lat0 + 200.0 / MET_PER_DEG_LAT, lng0),
        ];
        let (dense, arcs, _) = make_polyline_ring(ring, (lat0 + 100.0 / MET_PER_DEG_LAT, lng0 + 50.0 / MET_PER_DEG_LNG));
        let per = *arcs.last().unwrap();
        assert!((per - 600.0).abs() < 1.0, "周长={per}");
        // 弧长插值：s=150m 处于右边段（下边100m之后），x=+50m（相对中心），
        // 右边段从 y=-100m 向北走到 y=+100m，故 50m 处 y=-50m
        let (x, y) = ring_point_at(&dense, &arcs, 150.0);
        assert!((x - 50.0).abs() < 0.5, "x={x}");
        assert!((y + 50.0).abs() < 0.5, "y={y}");
    }

    /// 10 秒窗均值配速全部落在有效窗口内（判定规则 2'21"-10'00"/km），且总距精确。
    /// 逐点 avgSpeed 允许越界（真人爬坡期同样低于窗口，见 OBS 样本）。
    #[test]
    fn test_speeds_within_valid_pace_window() {
        let pts = sample_points();
        let combos = [
            (1050.0, 480i64),
            (1440.0, 661),
            (1920.0, 719),
            (2100.0, 900),
            (3300.0, 1220),
        ];
        for seed in 0..16u64 {
            for &(dist, dur) in &combos {
                let t = build(dist, dur, seed, (38.9, 121.54), 1_788_958_186_123, &pts);
                for (i, w) in t.speedPerTenSec.iter().enumerate() {
                    let pace = 1000.0 / (w.value / 10.0); // 秒/km
                    assert!(
                        (141.0..=600.0).contains(&pace),
                        "seed={seed} dist={dist} 窗{i} 配速 {}/km 越界",
                        format_args!("{}:{:02}", pace as i64 / 60, (pace as i64) % 60)
                    );
                }
                assert!(
                    (t.totalDistance - dist).abs() < 2.0,
                    "seed={seed} dist={}: {}",
                    dist,
                    t.totalDistance
                );
            }
        }
    }

    /// BD→GCJ 实测向量。
    #[test]
    fn test_bd09_to_gcj02_vector() {
        let (lat, lng) = bd09_to_gcj02(38.901678, 121.540241);
        assert!((lat - 38.8956025774013).abs() < 1e-9, "lat={lat}");
        assert!((lng - 121.5337497718317).abs() < 1e-9, "lng={lng}");
    }

    /// OBS 对象：10 键、gzip+base64 可解、run_data 27 键点集。
    #[test]
    fn test_obs_object_structure() {
        let pts: Vec<serde_json::Value> = sample_points()
            .iter()
            .enumerate()
            .map(|(i, (la, lo))| {
                serde_json::json!({
                    "lon": lo, "lat": la, "isFixed": 0,
                    "pointName": format!("P{i}"), "glon": lo - 0.006,
                    "glat": la - 0.006,
                })
            })
            .collect();
        let track = build(3300.0, 1220, 42, (38.9, 121.54), 1_788_958_186_123, &sample_points());
        let obj = build_obs_object(&track, 1320403809, "UUID-TEST", 13056447, &pts);
        let keys: Vec<&str> = obj.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "rrid", "uuid", "uid", "run_data", "fixed_point_json", "segment_json",
                "speed_json", "step_freq_json", "laps_json", "runFaceCheck"
            ]
        );
        // rrid gzip 可解
        let raw = crate::crypto::envelope::b64_decode(obj["rrid"].as_str().unwrap()).unwrap();
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        use std::io::Read;
        let mut s = String::new();
        dec.read_to_string(&mut s).unwrap();
        assert_eq!(s, "1320403809");
        // run_data 解包 → 27 键点集
        let raw = crate::crypto::envelope::b64_decode(obj["run_data"].as_str().unwrap()).unwrap();
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        let mut s = String::new();
        dec.read_to_string(&mut s).unwrap();
        let wrap: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(wrap["useZip"], false);
        let pts: Vec<serde_json::Value> =
            serde_json::from_str(wrap["allLocJson"].as_str().unwrap()).unwrap();
        assert_eq!(pts[0].as_object().unwrap().len(), 27, "点键数必须 27");
        // segment_json 是空串 gzip
        let raw = crate::crypto::envelope::b64_decode(obj["segment_json"].as_str().unwrap()).unwrap();
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        let mut s = String::new();
        dec.read_to_string(&mut s).unwrap();
        assert_eq!(s, "");
        // obs keys 两个
        let ks = obs_keys(&track, 1320403809, "UUID-TEST");
        assert_eq!(ks.len(), 2);
        assert!(ks[0].contains("run_data/"));
        assert!(ks[1].starts_with("run_data/1320/1320403809.json"));
    }

    /// 回归（上游 issue #26「记录达标但地图没有路线」）：
    /// ① 无服务端围栏时 area 不得携带伪围栏（geoFencesJson 必须为 "[]"）；
    /// ② 点型协议必须是详情页可识别的真人混合（无 -1 漂移、type 3 占比可观、
    ///    哨兵 5/6、首点 ∈{0,7}），直线环与道路环两种底环下都成立。
    #[test]
    fn test_route_display_protocol_no_pseudo_fence() {
        use crate::api::points::area_from_payload;
        use serde_json::json;

        // 无围栏字段的点位响应 → 空围栏（伪围栏会破坏整条路线解析）
        let payload = json!({"runAreaId": 9});
        let points = vec![json!({"lat": 1.0, "lon": 2.0}), json!({"lat": 1.1, "lon": 2.0}), json!({"lat": 1.1, "lon": 2.1})];
        let area = area_from_payload(&payload, &points);
        assert_eq!(area.geo_fences_json, "[]");
        assert!(!area.freedom_show_fence);
        assert_eq!(area.run_area_id, 9);

        // 非数组/空数组围栏一律拒绝
        assert_eq!(area_from_payload(&json!({"geoFencesJson": "not json"}), &[]).geo_fences_json, "[]");
        assert_eq!(area_from_payload(&json!({"geoFencesJson": "[]"}), &[]).geo_fences_json, "[]");
        assert_eq!(area_from_payload(&json!({"fences": {"lat": 1.0}}), &[]).geo_fences_json, "[]");
        // 合法数组围栏保留
        let ok = area_from_payload(&json!({"geoFencesJson": "[{\"lat\":1.0},{\"lat\":1.1}]"}), &[]);
        assert_ne!(ok.geo_fences_json, "[]");
        assert!(ok.freedom_show_fence);

        // 两种底环下点型协议一致
        let pts = sample_points();
        let order = super::roads::angular_order(&pts);
        let mut ring: Vec<(f64, f64)> = Vec::new();
        for &i in &order {
            ring.push(pts[i]);
        }
        ring.push(pts[order[0]]);
        let mut dense_ring = Vec::new();
        for w in ring.windows(2) {
            for j in 0..8 {
                let t = j as f64 / 8.0;
                dense_ring.push((w[0].0 + (w[1].0 - w[0].0) * t, w[1].1 * t + w[0].1 * (1.0 - t)));
            }
        }
        for seed in 0..8u64 {
            for (tag, track) in [
                ("直线环", build(2600.0, 960, seed, (38.9, 121.54), 1_788_958_186_123, &pts)),
                ("道路环", super::generator::build_with_ring(2600.0, 960, seed, (38.9, 121.54), 1_788_958_186_123, &pts, Some(&dense_ring))),
            ] {
                let locs = &track.locations;
                assert!(locs.iter().all(|p| p.ptype != -1), "{tag}: 漂移点未清除");
                assert!([0, 7].contains(&locs[0].ptype), "{tag}: 首点型 {}", locs[0].ptype);
                assert_eq!(locs[1].ptype, 5, "{tag}: 起点哨兵");
                assert_eq!(locs.last().unwrap().ptype, 6, "{tag}: 终点哨兵");
                // 真人 type=3 曲线点占比可观（详情页曲线路径识别）
                let t3 = locs.iter().filter(|p| p.ptype == 3).count();
                assert!(t3 * 100 >= locs.len() * 20, "{tag}: type3 占比 {t3}/{}", locs.len());
                // 圆滑半径（GPS 精度）全部处于真实范围
                assert!(locs.iter().all(|p| p.radius >= 1.4 && p.radius <= 5.1), "{tag}: radius 越界");
            }
        }
    }
}
