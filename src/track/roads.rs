//! 真实路网环线规划（避免轨迹直线穿越建筑与水面）。
//!
//! 直线拟合环假设相邻打卡点之间无遮挡；本模块改为沿 OpenStreetMap
//! 真实路网连接打卡点：Overpass API 拉取打卡点包围盒内的道路/步道
//! 要素（`out geom`），构图后用 Dijkstra 规划相邻打卡点间最短路径，
//! 拼接为闭合道路环。道路本身不会穿过建筑与湖泊，且打卡点作为图
//! 节点参与规划，保证轨迹仍精确经过全部打卡点。
//!
//! 任何一步失败都返回 `None`，由调用方回退到直线拟合环；
//! 路网响应按包围盒落盘缓存 7 天，避免重复请求触发限流。

use std::collections::{BinaryHeap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use super::geom::{MET_PER_DEG_LAT, MET_PER_DEG_LNG};
use super::wire::bd09_to_gcj02;

const X_PI: f64 = std::f64::consts::PI * 3000.0 / 180.0;
/// WGS-84 长半轴与第一偏心率平方（GCJ-02 偏移算法参数）。
const SEMI_MAJOR: f64 = 6_378_245.0;
const EE: f64 = 0.006_693_421_622_965_943_23;

const OVERPASS_ENDPOINTS: [&str; 2] = [
    "https://overpass.kumi.systems/api/interpreter",
    "https://overpass-api.de/api/interpreter",
];

/// 打卡点到路网节点的最大直线连接距离（米）。
const SNAP_MAX_M: f64 = 300.0;
/// 每个打卡点最多连接的最近路网节点数。
const SNAP_K: usize = 4;
/// 路网缓存有效期（秒）。
const CACHE_TTL: u64 = 7 * 24 * 3600;
/// 包围盒外扩（米）。
const BBOX_MARGIN_M: f64 = 250.0;
/// 道路环的最短长度（米），低于该值视为路网覆盖不足。
const MIN_RING_M: f64 = 300.0;

// ---------------------------------------------------------------------------
// 坐标变换
// ---------------------------------------------------------------------------

fn transform_lat(x: f64, y: f64) -> f64 {
    let mut ret = -100.0
        + 2.0 * x
        + 3.0 * y
        + 0.2 * y * y
        + 0.1 * x * y
        + 0.2 * x.abs().sqrt();
    ret += (20.0 * (6.0 * x * std::f64::consts::PI).sin() + 20.0 * (2.0 * x * std::f64::consts::PI).sin()) * 2.0 / 3.0;
    ret += (y * std::f64::consts::PI).sin() * 20.0 / 3.0
        + (2.0 * (y / 3.0) * std::f64::consts::PI).sin() * 20.0 / 3.0;
    ret += (y / 12.0 * std::f64::consts::PI).cos() * 160.0 / 3.0
        + (x * std::f64::consts::PI).sin() * 320.0 / 3.0;
    ret
}

fn transform_lng(x: f64, y: f64) -> f64 {
    let mut ret = 300.0 + x + 2.0 * y + 0.1 * x * x + 0.1 * x * y + 0.1 * x.abs().sqrt();
    ret += (20.0 * (6.0 * x * std::f64::consts::PI).sin() + 20.0 * (2.0 * x * std::f64::consts::PI).sin()) * 2.0 / 3.0;
    ret += (x * std::f64::consts::PI).sin() * 20.0 / 3.0
        + (2.0 * (x / 3.0) * std::f64::consts::PI).sin() * 20.0 / 3.0;
    ret += (x / 12.0 * std::f64::consts::PI).cos() * 150.0 / 3.0
        + (x / 30.0 * std::f64::consts::PI).cos() * 18.0 / 3.0;
    ret
}

fn out_of_china(lat: f64, lng: f64) -> bool {
    !(73.66..=135.05).contains(&lng) || !(3.86..=53.55).contains(&lat)
}

/// WGS-84 → GCJ-02 的偏移量（火星坐标系正向加偏）。
fn wgs84_offset(lat: f64, lng: f64) -> (f64, f64) {
    if out_of_china(lat, lng) {
        return (0.0, 0.0);
    }
    let dlat = transform_lat(lng - 105.0, lat - 35.0);
    let dlng = transform_lng(lng - 105.0, lat - 35.0);
    let radlat = lat / 180.0 * std::f64::consts::PI;
    let magic = 1.0 - EE * radlat.sin() * radlat.sin();
    let sqrtmagic = magic.sqrt();
    (
        dlat * 180.0 / ((SEMI_MAJOR * (1.0 - EE)) / (magic * sqrtmagic) * std::f64::consts::PI),
        dlng * 180.0 / (SEMI_MAJOR / sqrtmagic * radlat.cos() * std::f64::consts::PI),
    )
}

/// WGS-84 → GCJ-02。
pub fn wgs84_to_gcj02(wlat: f64, wlng: f64) -> (f64, f64) {
    let (dlat, dlng) = wgs84_offset(wlat, wlng);
    (wlat + dlat, wlng + dlng)
}

/// GCJ-02 → WGS-84（迭代近似逆变换，精度 ~1e-6°）。
pub fn gcj02_to_wgs84(glat: f64, glng: f64) -> (f64, f64) {
    let mut wlat = glat;
    let mut wlng = glng;
    for _ in 0..3 {
        let (dlat, dlng) = wgs84_offset(wlat, wlng);
        wlat = glat - dlat;
        wlng = glng - dlng;
    }
    (wlat, wlng)
}

/// GCJ-02 → 百度 BD-09（[wire::bd09_to_gcj02] 的逆）。
pub fn gcj02_to_bd09(glat: f64, glng: f64) -> (f64, f64) {
    let z = (glat * glat + glng * glng).sqrt() + 0.00002 * (glat * X_PI).sin();
    let theta = glat.atan2(glng) + 0.000003 * (glng * X_PI).cos();
    (z * theta.sin() + 0.006, z * theta.cos() + 0.0065)
}

// ---------------------------------------------------------------------------
// 几何工具
// ---------------------------------------------------------------------------

/// 等距近似两点距离（米）。与生成器共用同一组每度米数常量，保持一致性。
fn dist_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dy = (a.0 - b.0) * MET_PER_DEG_LAT;
    let dx = (a.1 - b.1) * MET_PER_DEG_LNG;
    (dx * dx + dy * dy).sqrt()
}

/// 按对中心点的极角排序打卡点下标（与 geom::make_point_ring 同序）。
pub fn angular_order(pts: &[(f64, f64)]) -> Vec<usize> {
    let n = pts.len();
    let clat = pts.iter().map(|p| p.0).sum::<f64>() / n as f64;
    let clng = pts.iter().map(|p| p.1).sum::<f64>() / n as f64;
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let ka = (pts[a].0 - clat).atan2(pts[a].1 - clng);
        let kb = (pts[b].0 - clat).atan2(pts[b].1 - clng);
        ka.partial_cmp(&kb).unwrap()
    });
    order
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ---------------------------------------------------------------------------
// 路网图
// ---------------------------------------------------------------------------

struct RoadGraph {
    /// 节点坐标 WGS (lat, lng)。
    coords: Vec<(f64, f64)>,
    adj: Vec<Vec<(usize, f64)>>,
    key_of: HashMap<(i64, i64), usize>,
    is_checkpoint: Vec<bool>,
}

impl RoadGraph {
    fn new() -> Self {
        Self {
            coords: Vec::new(),
            adj: Vec::new(),
            key_of: HashMap::new(),
            is_checkpoint: Vec::new(),
        }
    }

    fn node_key(p: (f64, f64)) -> (i64, i64) {
        ((p.0 * 1e7).round() as i64, (p.1 * 1e7).round() as i64)
    }

    /// 取节点（不存在则创建）。
    fn node(&mut self, p: (f64, f64)) -> usize {
        let key = Self::node_key(p);
        if let Some(&i) = self.key_of.get(&key) {
            return i;
        }
        let i = self.coords.len();
        self.coords.push(p);
        self.adj.push(Vec::new());
        self.is_checkpoint.push(false);
        self.key_of.insert(key, i);
        i
    }

    fn edge(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        let w = dist_m(self.coords[a], self.coords[b]);
        self.adj[a].push((b, w));
        self.adj[b].push((a, w));
    }

    /// 把打卡点注册为图节点，向 SNAP_MAX_M 内最近的 SNAP_K 个道路节点连边。
    /// 打卡点不与其它打卡点直连（避免绕过路网的直线捷径）。
    fn attach_checkpoint(&mut self, p: (f64, f64)) -> Option<usize> {
        let road_nodes: Vec<usize> = (0..self.coords.len())
            .filter(|&i| !self.is_checkpoint[i])
            .collect();
        let mut cands: Vec<(f64, usize)> = road_nodes
            .into_iter()
            .map(|i| (dist_m(p, self.coords[i]), i))
            .filter(|&(d, _)| d <= SNAP_MAX_M)
            .collect();
        cands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if cands.is_empty() {
            return None;
        }
        let cp = self.node(p);
        self.is_checkpoint[cp] = true;
        for (_, i) in cands.into_iter().take(SNAP_K) {
            self.edge(cp, i);
        }
        Some(cp)
    }

    /// Dijkstra 最短路（距离，米；路径含起终点）。
    fn route(&self, src: usize, dst: usize) -> Option<(f64, Vec<usize>)> {
        let n = self.coords.len();
        let mut dist = vec![f64::INFINITY; n];
        let mut prev = vec![usize::MAX; n];
        let mut heap = BinaryHeap::new();
        dist[src] = 0.0;
        heap.push((std::cmp::Reverse(0u64), src));
        while let Some((Reverse(d_cm), u)) = heap.pop() {
            if u == dst {
                break;
            }
            if d_cm as f64 / 100.0 > dist[u] {
                continue;
            }
            for &(v, w) in &self.adj[u] {
                let nd = dist[u] + w;
                if nd < dist[v] {
                    dist[v] = nd;
                    prev[v] = u;
                    heap.push((std::cmp::Reverse((nd * 100.0) as u64), v));
                }
            }
        }
        if !dist[dst].is_finite() {
            return None;
        }
        let mut path = vec![dst];
        let mut cur = dst;
        while cur != src {
            cur = prev[cur];
            path.push(cur);
        }
        path.reverse();
        Some((dist[dst], path))
    }
}

/// 从 Overpass 响应 JSON 构建路网图。
fn build_graph(body: &str) -> Option<RoadGraph> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let elements = v.get("elements")?.as_array()?;
    let mut graph = RoadGraph::new();
    for way in elements {
        if way.get("type").and_then(|t| t.as_str()) != Some("way") {
            continue;
        }
        // 缺 geometry 的要素直接跳过，不能中止整个建图流程
        let geometry = match way.get("geometry").and_then(|g| g.as_array()) {
            Some(g) if !g.is_empty() => g,
            _ => continue,
        };
        let nodes: Vec<usize> = geometry
            .iter()
            .filter_map(|pt| {
                let la = pt.get("lat")?.as_f64()?;
                let lo = pt.get("lon")?.as_f64()?;
                Some(graph.node((la, lo)))
            })
            .collect();
        for w in nodes.windows(2) {
            graph.edge(w[0], w[1]);
        }
    }
    // 路网过稀（<8 节点）没有规划价值，直接回退直线环。
    if graph.coords.len() < 8 {
        return None;
    }
    Some(graph)
}

// ---------------------------------------------------------------------------
// Overpass 请求与缓存
// ---------------------------------------------------------------------------

/// Overpass 查询：包围盒内除快速路外的全部道路/步道要素。
fn overpass_query(bbox: (f64, f64, f64, f64)) -> String {
    format!(
        "[out:json][timeout:25];(way[\"highway\"][\"highway\"!~\"^(motorway|trunk|motorway_link|trunk_link|construction|proposed|raceway)$\"]({},{},{},{}););out geom;",
        bbox.0, bbox.1, bbox.2, bbox.3
    )
}

fn cache_path(bbox: (f64, f64, f64, f64)) -> std::path::PathBuf {
    let key = format!("{:.6},{:.6},{:.6},{:.6}", bbox.0, bbox.1, bbox.2, bbox.3);
    crate::platform::data_dir().join(format!("roadnet-{:016x}.json", fnv1a(&key)))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 读缓存（7 天有效）。
fn read_cache(bbox: (f64, f64, f64, f64)) -> Option<String> {
    let path = cache_path(bbox);
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let saved = v.get("saved")?.as_u64()?;
    if now_secs().saturating_sub(saved) > CACHE_TTL {
        return None;
    }
    let body = v.get("body")?.as_str()?.to_string();
    if body.is_empty() {
        return None;
    }
    Some(body)
}

fn write_cache(bbox: (f64, f64, f64, f64), body: &str) {
    let v = serde_json::json!({ "saved": now_secs(), "body": body });
    let _ = std::fs::write(cache_path(bbox), v.to_string());
}

/// 依次尝试多个 Overpass 镜像拉取路网；成功响应写入缓存。
fn fetch_overpass(bbox: (f64, f64, f64, f64), log: &mut dyn FnMut(&str)) -> Option<String> {
    if let Some(cached) = read_cache(bbox) {
        log("√ [roads] 命中路网缓存");
        return Some(cached);
    }
    let query = overpass_query(bbox);
    let agent = crate::api::client::make_agent();
    for url in OVERPASS_ENDPOINTS {
        log(&format!("[roads] 请求 Overpass 路网 {url} …"));
        let result = agent
            .post(url)
            .timeout(std::time::Duration::from_secs(40))
            .set("Content-Type", "text/plain")
            .send_string(&query);
        match result {
            Ok(resp) => match resp.into_string() {
                Ok(body) if !body.is_empty() => {
                    write_cache(bbox, &body);
                    log("√ [roads] 路网拉取成功（已缓存 7 天）");
                    return Some(body);
                }
                Ok(_) => log("[roads] Overpass 返回空响应"),
                Err(e) => log(&format!("[roads] 读取响应失败：{e}")),
            },
            Err(e) => log(&format!("[roads] 请求失败：{e}")),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 主入口
// ---------------------------------------------------------------------------

/// 规划沿真实路网的闭合环线（BD 系坐标）。
///
/// 输入为打卡点 BD 坐标；输出为稠密闭合环，可直接交给
/// [super::geom::make_polyline_ring] 作为轨迹底环。
/// 打卡点稀少、路网不可达或覆盖不足时返回 None。
pub fn plan_road_ring(pts_bd: &[(f64, f64)], log: &mut dyn FnMut(&str)) -> Option<Vec<(f64, f64)>> {
    if pts_bd.len() < 3 {
        return None;
    }
    // ① BD → GCJ → WGS（OSM 使用 WGS-84）
    let wgs: Vec<(f64, f64)> = pts_bd
        .iter()
        .map(|&(la, lo)| {
            let (gla, glo) = bd09_to_gcj02(la, lo);
            gcj02_to_wgs84(gla, glo)
        })
        .collect();
    // ② 打卡点按极角排序（与直线环同序，保证打卡顺序一致）
    let order = angular_order(&wgs);
    let wgs: Vec<(f64, f64)> = order.iter().map(|&i| wgs[i]).collect();
    // ③ 包围盒（外扩 BBOX_MARGIN_M）
    let (min_lat, max_lat) = wgs.iter().fold((f64::MAX, f64::MIN), |acc, p| (acc.0.min(p.0), acc.1.max(p.0)));
    let (min_lng, max_lng) = wgs.iter().fold((f64::MAX, f64::MIN), |acc, p| (acc.0.min(p.1), acc.1.max(p.1)));
    let mlat = BBOX_MARGIN_M / MET_PER_DEG_LAT;
    let mlng = BBOX_MARGIN_M / MET_PER_DEG_LNG;
    let bbox = (min_lat - mlat, min_lng - mlng, max_lat + mlat, max_lng + mlng);
    // ④ 缓存/请求路网
    let body = fetch_overpass(bbox, log)?;
    // ⑤ 建图 + 打卡点入图
    let mut graph = build_graph(&body)?;
    log(&format!("√ [roads] 路网 {} 节点", graph.coords.len()));
    let mut nodes = Vec::with_capacity(wgs.len());
    for &p in &wgs {
        match graph.attach_checkpoint(p) {
            Some(n) => nodes.push(n),
            None => {
                log("[roads] 存在超过 300m 无法接入路网的打卡点，回退直线环");
                return None;
            }
        }
    }
    // ⑥ 相邻打卡点间最短路拼接闭合环
    let mut ring_wgs: Vec<(f64, f64)> = Vec::new();
    let mut total = 0.0f64;
    for i in 0..nodes.len() {
        let a = nodes[i];
        let b = nodes[(i + 1) % nodes.len()];
        let (d, path) = graph.route(a, b)?;
        total += d;
        for &n in &path {
            let p = graph.coords[n];
            // 相邻段共享打卡点端点，靠距离去重
            if let Some(&q) = ring_wgs.last() {
                if dist_m(q, p) < 0.05 {
                    continue;
                }
            }
            ring_wgs.push(p);
        }
    }
    // 首尾闭合：去掉与起点重复的收尾点
    while ring_wgs.len() > 2 && dist_m(ring_wgs[0], *ring_wgs.last().unwrap()) < 0.05 {
        ring_wgs.pop();
    }
    if total < MIN_RING_M || ring_wgs.len() < 8 {
        log(&format!("[roads] 道路环过短（{total:.0}m / {} 点），回退直线环", ring_wgs.len()));
        return None;
    }
    log(&format!(
        "√ [roads] 道路环 {:.0}m / {} 顶点，轨迹将沿真实道路生成",
        total,
        ring_wgs.len()
    ));
    // ⑦ WGS → GCJ → BD
    Some(
        ring_wgs
            .iter()
            .map(|&(la, lo)| {
                let (gla, glo) = wgs84_to_gcj02(la, lo);
                gcj02_to_bd09(gla, glo)
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gcj_wgs_roundtrip() {
        // BD→GCJ 已有实测基线，这里验证 GCJ↔WGS 往返收敛
        let (glat, glng) = (38.8956025774013, 121.5337497718317);
        let (wlat, wlng) = gcj02_to_wgs84(glat, glng);
        let (glat2, glng2) = wgs84_to_gcj02(wlat, wlng);
        assert!((glat2 - glat).abs() < 1e-5, "lat {glat} vs {glat2}");
        assert!((glng2 - glng).abs() < 1e-5, "lng {glng} vs {glng2}");
    }

    #[test]
    fn test_bd_gcj_wgs_bd_roundtrip() {
        let (bd_lat, bd_lng) = (38.901678, 121.540241);
        let (gla, glo) = bd09_to_gcj02(bd_lat, bd_lng);
        let (wla, wlo) = gcj02_to_wgs84(gla, glo);
        let (gla2, glo2) = wgs84_to_gcj02(wla, wlo);
        let (bd2, bd3) = gcj02_to_bd09(gla2, glo2);
        assert!((bd2 - bd_lat).abs() < 1e-4, "{bd2} vs {bd_lat}");
        assert!((bd3 - bd_lng).abs() < 1e-4, "{bd3} vs {bd_lng}");
    }

    #[test]
    fn test_angular_order_matches_ring_convention() {
        // 正方形四角应按极角（atan2(dLat,dLng)）升序
        let pts = vec![
            (39.0, 116.0),
            (39.1, 116.1),
            (39.0, 116.1),
            (39.1, 116.0),
        ];
        let order = angular_order(&pts);
        assert_eq!(order.len(), 4);
        let angles: Vec<f64> = order
            .iter()
            .map(|&i| (pts[i].0 - 39.05).atan2(pts[i].1 - 116.05))
            .collect();
        let mut sorted = angles.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for (a, b) in angles.iter().zip(sorted.iter()) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn test_graph_route_prefers_short_path() {
        let mut g = RoadGraph::new();
        // 三角形：A-B 直连 100m，A-C-B 绕行 200m
        let a = g.node((39.00000, 116.00000));
        let b = g.node((39.00000, 116.00116));
        let c = g.node((39.00090, 116.00058));
        g.edge(a, b);
        g.edge(a, c);
        g.edge(c, b);
        let (d, path) = g.route(a, b).expect("路径可达");
        assert!(path.len() == 2, "应走 A→B 直连，path={path:?}");
        assert!(d < 150.0, "d={d}");
        // 断开直连后应绕行
        g.adj[a].retain(|&(v, _)| v != b);
        g.adj[b].retain(|&(v, _)| v != a);
        let (d2, path2) = g.route(a, b).expect("绕行可达");
        assert_eq!(path2[1], c, "应绕行 C");
        assert!(d2 > d, "绕行更远：d2={d2} d={d}");
    }

    #[test]
    fn test_build_graph_parses_overpass_sample() {
        let body = serde_json::json!({
            "elements": [
                { "type": "way", "id": 1, "geometry": [
                    { "lat": 39.0, "lon": 116.0 },
                    { "lat": 39.001, "lon": 116.0 }
                ]},
                { "type": "way", "id": 2, "geometry": [
                    { "lat": 39.001, "lon": 116.0 },
                    { "lat": 39.002, "lon": 116.001 }
                ]},
                { "type": "node", "id": 9, "lat": 39.0, "lon": 116.0 }
            ]
        })
        .to_string();
        let g = build_graph(&body).expect("应解析出图");
        assert_eq!(g.coords.len(), 3, "节点去重后 3 个");
        // 两条 way 在 (39.001,116.0) 共享节点，构成 3 节点路径
        let (d, path) = g.route(0, 2).expect("可达");
        assert!(d > 100.0, "d={d}");
        assert_eq!(path, vec![0, 1, 2]);
    }

    #[test]
    fn test_build_graph_rejects_sparse() {
        let body = serde_json::json!({
            "elements": [
                { "type": "way", "id": 1, "geometry": [
                    { "lat": 39.0, "lon": 116.0 },
                    { "lat": 39.001, "lon": 116.0 }
                ]}
            ]
        })
        .to_string();
        assert!(build_graph(&body).is_none(), "2 节点路网应回退直线环");
    }

    #[test]
    fn test_overpass_query_contains_bbox() {
        let q = overpass_query((38.895, 121.533, 38.905, 121.545));
        assert!(q.contains("(38.895,121.533,38.905,121.545)"));
        assert!(q.contains("out geom"));
        assert!(q.contains("motorway"));
    }
}
