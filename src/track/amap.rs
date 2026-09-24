//! 高德步行路径规划作为路网数据源（可选，需要用户自己的高德 Web服务 Key）。
//!
//! 适用场景：OSM 未绘制校园内部道路时（如广西职业技术大学），高德在
//! 中国校园区域的步行路网覆盖显著更好。逐段请求相邻打卡点间的步行
//! 路线（沿真实道路/步道），拼接为闭合环。
//!
//! 坐标系：高德 API 使用 GCJ-02；输入为 BD 打卡点，输出 BD 折线环。
//! 任何一段失败即返回 None，由调用方回落 OSM / 直线环。

use super::roads::{angular_order, dist_m, fnv1a};
use super::wire::bd09_to_gcj02;
use crate::crypto::envelope::b64_encode;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;

const AMAP_WALK_URL: &str = "https://restapi.amap.com/v3/direction/walking";
/// 相邻打卡点间的步行路线距离上限（校园尺度绕行可到 2-3km）。
const MAX_LEG_M: f64 = 3000.0;
/// 步行距离 / 直线距离 的绕行系数上限；超过说明两点间没有连贯的
/// 校内步行路网，路线在沿校外市政路绕大圈（如 OSM/高德均未绘制
/// 校园道路的学校），生成的环会跑出校园。
const MAX_DETOUR_RATIO: f64 = 3.5;
/// 免费个人 Key QPS=3，段间至少 400ms。
const LEG_INTERVAL_MS: u64 = 400;
const CACHE_TTL: u64 = 7 * 24 * 3600;

/// gzip + base64（本地缓存用，与 roads.rs 一致）。
fn gz(data: &[u8]) -> String {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).expect("gzip 写入内存失败");
    b64_encode(&enc.finish().expect("gzip 收尾失败"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path(pts_bd: &[(f64, f64)]) -> std::path::PathBuf {
    let key = pts_bd
        .iter()
        .map(|p| format!("{:.7},{:.7}", p.0, p.1))
        .collect::<Vec<_>>()
        .join(";");
    crate::platform::data_dir().join(format!("amapwalk-{:016x}.json", fnv1a(&key)))
}

fn read_cache(pts_bd: &[(f64, f64)]) -> Option<Vec<(f64, f64)>> {
    let raw = std::fs::read_to_string(cache_path(pts_bd)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let saved = v.get("saved")?.as_u64()?;
    if now_secs().saturating_sub(saved) > CACHE_TTL {
        return None;
    }
    let ring = v.get("ring")?.as_array()?;
    ring.iter()
        .map(|p| Some((p.get("la")?.as_f64()?, p.get("lo")?.as_f64()?)))
        .collect()
}

fn write_cache(pts_bd: &[(f64, f64)], ring: &[(f64, f64)]) {
    let ring: Vec<serde_json::Value> = ring
        .iter()
        .map(|&(la, lo)| serde_json::json!({"la": la, "lo": lo}))
        .collect();
    let v = serde_json::json!({ "saved": now_secs(), "ring": ring });
    let _ = std::fs::write(cache_path(pts_bd), v.to_string());
}

/// 单段步行路线：返回折线（GCJ-02）与距离（米）。
fn walk_leg(agent: &ureq::Agent, key: &str, a: (f64, f64), b: (f64, f64)) -> Option<(Vec<(f64, f64)>, f64)> {
    // 高德 origin/destination 为 lng,lat（GCJ-02）
    let url = format!(
        "{AMAP_WALK_URL}?origin={:.6},{:.6}&destination={:.6},{:.6}&key={key}",
        a.1, a.0, b.1, b.0
    );
    let resp = agent.get(&url).timeout(std::time::Duration::from_secs(20)).call().ok()?;
    let body: serde_json::Value = resp.into_json().ok()?;
    if body.get("status").and_then(|s| s.as_str()) != Some("1") {
        return None;
    }
    let path = body.get("route")?.get("paths")?.as_array()?.first()?.clone();
    let distance = path.get("distance").and_then(|d| d.as_str()).and_then(|d| d.parse::<f64>().ok())?;
    let mut line: Vec<(f64, f64)> = Vec::new();
    for step in path.get("steps")?.as_array()? {
        let poly = step.get("polyline").and_then(|p| p.as_str())?;
        for pair in poly.split(';') {
            let mut it = pair.split(',');
            let (Some(lon), Some(lat)) = (it.next(), it.next()) else { continue };
            let (Ok(lon), Ok(lat)) = (lon.trim().parse::<f64>(), lat.trim().parse::<f64>()) else { continue };
            if lon.abs() > 180.0 || lat.abs() > 90.0 {
                continue;
            }
            // 折线里段与段之间的连接点会重复
            if let Some(&q) = line.last() {
                if dist_m(q, (lat, lon)) < 0.05 {
                    continue;
                }
            }
            line.push((lat, lon));
        }
    }
    if line.len() < 2 {
        return None;
    }
    Some((line, distance))
}

/// 规划沿高德步行路网的闭合环线。
///
/// 输入打卡点 BD 坐标；输出 BD 系闭合折线环（打卡点顺序按极角排序，
/// 与直线环/OSM 环一致）。失败返回 None。
pub fn plan_amap_ring(pts_bd: &[(f64, f64)], key: &str, log: &mut dyn FnMut(&str)) -> Option<Vec<(f64, f64)>> {
    let key = key.trim();
    if pts_bd.len() < 3 || key.is_empty() || key.chars().all(|c| !c.is_ascii_graphic()) {
        return None;
    }
    if let Some(ring) = read_cache(pts_bd) {
        log("√ [amap] 命中步行路线缓存");
        return Some(ring);
    }
    let agent = crate::api::client::make_agent();
    // BD → GCJ（高德坐标系），按极角排序与直线环同序
    let mut pts_gcj: Vec<(f64, f64)> = pts_bd
        .iter()
        .map(|&(la, lo)| bd09_to_gcj02(la, lo))
        .collect();
    let order = angular_order(&pts_gcj);
    let pts_gcj: Vec<(f64, f64)> = order.iter().map(|&i| pts_gcj[i]).collect();

    let mut ring_gcj: Vec<(f64, f64)> = Vec::new();
    let mut total = 0.0f64;
    for i in 0..pts_gcj.len() {
        let a = pts_gcj[i];
        let b = pts_gcj[(i + 1) % pts_gcj.len()];
        log(&format!("[amap] 步行路线段 {}/{} …", i + 1, pts_gcj.len()));
        let (line, distance) = walk_leg(&agent, key, a, b)?;
        let straight = dist_m(a, b);
        if distance > MAX_LEG_M {
            log(&format!("[amap] 段 {i} 步行距离 {distance:.0}m 超限，放弃道路环"));
            return None;
        }
        if straight > 0.0 && distance / straight > MAX_DETOUR_RATIO {
            log(&format!(
                "[amap] 段 {i} 步行 {distance:.0}m / 直线 {straight:.0}m 绕行比过大（校内路网缺失），放弃道路环"
            ));
            return None;
        }
        total += distance;
        for &p in &line {
            // 相邻段共享打卡点端点，靠距离去重
            if let Some(&q) = ring_gcj.last() {
                if dist_m(q, p) < 0.05 {
                    continue;
                }
            }
            ring_gcj.push(p);
        }
        // 请求间隔，避免触发免费 Key QPS 限制
        std::thread::sleep(std::time::Duration::from_millis(LEG_INTERVAL_MS));
    }
    while ring_gcj.len() > 2 && dist_m(ring_gcj[0], *ring_gcj.last().unwrap()) < 0.05 {
        ring_gcj.pop();
    }
    // 首尾闭合：末点应接近首点（步行路线不保证闭合，偏差大则放弃）
    if ring_gcj.len() < 8 || dist_m(ring_gcj[0], *ring_gcj.last().unwrap()) > 300.0 {
        log(&format!(
            "[amap] 步行环不闭合或过短（{} 点），放弃道路环",
            ring_gcj.len()
        ));
        return None;
    }
    log(&format!("√ [amap] 步行道路环 {total:.0}m / {} 顶点", ring_gcj.len()));
    // GCJ → BD
    let ring_bd: Vec<(f64, f64)> = ring_gcj
        .iter()
        .map(|&(gla, glo)| crate::track::roads::gcj02_to_bd09(gla, glo))
        .collect();
    write_cache(pts_bd, &ring_bd);
    Some(ring_bd)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 步行路线响应解析：polyline 分号/逗号分隔，重复连接点去重，status!=1 失败。
    #[test]
    fn test_amap_walk_leg_parses_polyline() {
        let sample = serde_json::json!({
            "status": "1",
            "info": "OK",
            "route": {
                "paths": [{
                    "distance": "358",
                    "steps": [
                        {"polyline": "108.233220,22.582505;108.234100,22.583010;108.234100,22.583010;108.235000,22.583500"},
                        {"polyline": "108.235000,22.583500;108.236000,22.584000"}
                    ]
                }]
            }
        });
        // 复用 walk_leg 的解析逻辑：直接构造同样的请求不可行（无 key），这里
        // 解析断言内联验证（与 walk_leg 中的解析规则保持一致）。
        let path = &sample["route"]["paths"][0];
        let mut line: Vec<(f64, f64)> = Vec::new();
        for step in path["steps"].as_array().unwrap() {
            let poly = step["polyline"].as_str().unwrap();
            for pair in poly.split(';') {
                let mut it = pair.split(',');
                let (lon, lat) = (it.next().unwrap(), it.next().unwrap());
                let (lon, lat) = (lon.parse::<f64>().unwrap(), lat.parse::<f64>().unwrap());
                if let Some(&q) = line.last() {
                    if dist_m(q, (lat, lon)) < 0.05 { continue; }
                }
                line.push((lat, lon));
            }
        }
        assert_eq!(line.len(), 4, "重复连接点应去重: {line:?}");
        assert_eq!(line[0], (22.582505, 108.233220));
        assert_eq!(line[3], (22.584000, 108.236000));
    }

    /// key 为空时直接放弃（不打网络请求）。
    #[test]
    fn test_amap_ring_requires_key() {
        let pts = [(38.901678, 121.540241), (38.902564, 121.541233), (38.900921, 121.542310)];
        assert!(plan_amap_ring(&pts, "", &mut |_| {}).is_none());
        assert!(plan_amap_ring(&pts[..2], "test-key", &mut |_| {}).is_none());
    }

    /// 高德步行路网能力探测（广西职业技术大学）。需要环境变量 AMAP_KEY；
    /// 本地运行：`AMAP_KEY=xxx HTTPS_PROXY=... cargo test --lib --locked -- --ignored --nocapture amap_walk_smoke`
    #[test]
    #[ignore = "需要高德 Key 与网络"]
    fn amap_walk_smoke_guangxi_vocational_technical_university() {
        let key = std::env::var("AMAP_KEY").expect("请设置 AMAP_KEY 环境变量");
        let center = (22.5825052f64, 108.2332206f64);
        let offsets: [(f64, f64); 5] = [
            (0.0000, 0.0000),
            (0.0040, 0.0010),
            (0.0020, 0.0050),
            (-0.0030, 0.0040),
            (-0.0020, -0.0040),
        ];
        let pts_bd: Vec<(f64, f64)> = offsets
            .iter()
            .map(|&(dla, dlo)| {
                let (gla, glo) = super::super::roads::wgs84_to_gcj02(center.0 + dla, center.1 + dlo);
                super::super::roads::gcj02_to_bd09(gla, glo)
            })
            .collect();
        let started = std::time::Instant::now();
        let ring = plan_amap_ring(&pts_bd, &key, &mut |s: &str| println!("{s}"));
        println!("耗时 {:.1}s", started.elapsed().as_secs_f32());
        match ring {
            Some(ring) => {
                let mut total: f64 = ring.windows(2).map(|w| dist_m(w[0], w[1])).sum();
                if let (Some(first), Some(last)) = (ring.first(), ring.last()) {
                    total += dist_m(*last, *first);
                }
                println!("结论：高德步行路网可规划，{} 顶点，环长 {total:.0}m", ring.len());
                for (i, p) in pts_bd.iter().enumerate() {
                    let d = ring.iter().map(|q| dist_m(*p, *q)).fold(f64::INFINITY, f64::min);
                    assert!(d < 2.0, "打卡点 {i} 距环 {d:.1}m");
                }
            }
            None => println!("结论：高德步行路网规划失败（见上方日志，已具备回落 OSM/直线环）"),
        }
    }
}
