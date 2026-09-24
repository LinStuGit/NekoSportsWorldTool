//! 校园内置离线路网环线规划（避免轨迹直线穿越建筑与水面）。
//!
//! 软件仅支持广西职业技术大学（南宁市江南区明阳大道19号），其校园
//! 步行路网已离线采集并内嵌为 [assets/gxvtu_net.json]（GCJ-02 坐标，
//! 297 节点 / 397 边，采集自高德步行路径规划，主连通分量 + 共线简化）。
//! 规划时把打卡点注册为图节点，沿内置路网用 Dijkstra 连接相邻打卡点，
//! 拼接为闭合道路环——道路本身不会穿过建筑与湖泊。
//!
//! 正式软件不做任何路网 API 调用（无 Overpass / 无高德请求）。
//! 打卡点超出内置路网贴靠范围或不可达时返回 `None`，由调用方回退到
//! 直线拟合环。

use std::collections::{BinaryHeap, HashMap};

use super::geom::MET_PER_DEG_LAT;
use super::geom::MET_PER_DEG_LNG;
use super::wire::bd09_to_gcj02;

const X_PI: f64 = std::f64::consts::PI * 3000.0 / 180.0;
/// WGS-84 长半轴与第一偏心率平方（GCJ-02 偏移算法参数）。
const SEMI_MAJOR: f64 = 6_378_245.0;
const EE: f64 = 0.006_693_421_622_965_943_23;

/// 打卡点到路网节点的最大直线连接距离（米）。
const SNAP_MAX_M: f64 = 300.0;
/// 每个打卡点最多连接的最近路网节点数。
const SNAP_K: usize = 4;
/// 道路环的最短长度（米），低于该值视为路网覆盖不足。
const MIN_RING_M: f64 = 300.0;

/// 内嵌校园路网（GCJ-02，[lng, lat] 节点 + 节点对边表）。
const CAMPUS_NET_JSON: &str = include_str!("../../assets/gxvtu_net.json");

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
        + (x / 3.0 * std::f64::consts::PI).sin() * 20.0 / 3.0;
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
pub(crate) fn dist_m(a: (f64, f64), b: (f64, f64)) -> f64 {
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

// ---------------------------------------------------------------------------
// 路网图
// ---------------------------------------------------------------------------

struct RoadGraph {
    /// 节点坐标 GCJ (lat, lng)。
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
        while let Some((std::cmp::Reverse(d_cm), u)) = heap.pop() {
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

/// 解析内嵌校园路网 JSON（GCJ-02）为图。
fn build_graph(body: &str) -> Option<RoadGraph> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let nodes = v.get("nodes")?.as_array()?;
    let edges = v.get("edges")?.as_array()?;
    let mut graph = RoadGraph::new();
    for n in nodes {
        let pair = n.as_array()?;
        let lng = pair.first()?.as_f64()?;
        let lat = pair.get(1)?.as_f64()?;
        graph.node((lat, lng));
    }
    for e in edges {
        let pair = e.as_array()?;
        let a = pair.first()?.as_u64()? as usize;
        let b = pair.get(1)?.as_u64()? as usize;
        if a >= graph.coords.len() || b >= graph.coords.len() {
            return None;
        }
        graph.edge(a, b);
    }
    // 路网过稀（<8 节点）没有规划价值，直接回退直线环。
    if graph.coords.len() < 8 {
        return None;
    }
    Some(graph)
}

// ---------------------------------------------------------------------------
// 主入口
// ---------------------------------------------------------------------------

/// 规划沿内置校园路网的闭合环线（BD 系坐标）。
///
/// 输入为打卡点 BD 坐标；输出为稠密闭合环，可直接交给
/// [super::geom::make_polyline_ring] 作为轨迹底环。
/// 打卡点稀少、超出路网贴靠范围或不可达时返回 None。
pub fn plan_road_ring(pts_bd: &[(f64, f64)], log: &mut dyn FnMut(&str)) -> Option<Vec<(f64, f64)>> {
    if pts_bd.len() < 3 {
        return None;
    }
    // ① BD → GCJ（内置路网为 GCJ-02 坐标）
    let gcj: Vec<(f64, f64)> = pts_bd.iter().map(|&(la, lo)| bd09_to_gcj02(la, lo)).collect();
    // ② 打卡点按极角排序（与直线环同序，保证打卡顺序一致）
    let order = angular_order(&gcj);
    let gcj: Vec<(f64, f64)> = order.iter().map(|&i| gcj[i]).collect();
    // ③ 解析内置路网 + 打卡点入图
    let mut graph = build_graph(CAMPUS_NET_JSON)?;
    log(&format!(
        "√ [roads] 内置校园路网 {} 节点 / {} 边",
        graph.coords.len(),
        graph.adj.iter().map(|a| a.len()).sum::<usize>() / 2
    ));
    let mut nodes = Vec::with_capacity(gcj.len());
    for &p in &gcj {
        match graph.attach_checkpoint(p) {
            Some(n) => nodes.push(n),
            None => {
                log("[roads] 存在超过 300m 无法接入校园路网的打卡点，回退直线环");
                return None;
            }
        }
    }
    // ④ 相邻打卡点间最短路拼接闭合环
    let mut ring_gcj: Vec<(f64, f64)> = Vec::new();
    let mut total = 0.0f64;
    for i in 0..nodes.len() {
        let a = nodes[i];
        let b = nodes[(i + 1) % nodes.len()];
        let (d, path) = graph.route(a, b)?;
        total += d;
        for &n in &path {
            let p = graph.coords[n];
            // 相邻段共享打卡点端点，靠距离去重
            if let Some(&q) = ring_gcj.last() {
                if dist_m(q, p) < 0.05 {
                    continue;
                }
            }
            ring_gcj.push(p);
        }
    }
    // 首尾闭合：去掉与起点重复的收尾点
    while ring_gcj.len() > 2 && dist_m(ring_gcj[0], *ring_gcj.last().unwrap()) < 0.05 {
        ring_gcj.pop();
    }
    if total < MIN_RING_M || ring_gcj.len() < 8 {
        log(&format!("[roads] 道路环过短（{total:.0}m / {} 点），回退直线环", ring_gcj.len()));
        return None;
    }
    log(&format!(
        "√ [roads] 道路环 {:.0}m / {} 顶点，轨迹将沿校园真实道路生成",
        total,
        ring_gcj.len()
    ));
    // ⑤ GCJ → BD
    Some(ring_gcj.iter().map(|&(la, lo)| gcj02_to_bd09(la, lo)).collect())
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 校园真实打卡点近似位置（GCJ-02，高德 POI）。
    const POIS_GCJ: [(&str, f64, f64); 6] = [
        ("图书馆", 108.238633, 22.579573),
        ("学生宿舍二区", 108.237512, 22.580661),
        ("第一食堂", 108.233775, 22.579175),
        ("体育场", 108.235303, 22.578041),
        ("茶叶实训中心", 108.238401, 22.577899),
        ("第二食堂", 108.235879, 22.581115),
    ];

    /// 内置路网解析：节点/边规模与坐标范围必须落在校园内。
    #[test]
    fn test_embedded_campus_network() {
        let g = build_graph(CAMPUS_NET_JSON).expect("内置路网应可解析");
        assert!(g.coords.len() >= 200, "节点数 {}", g.coords.len());
        let edges: usize = g.adj.iter().map(|a| a.len()).sum::<usize>() / 2;
        assert!(edges >= 200, "边数 {edges}");
        // 校园范围（GCJ-02）：lng 108.232~108.241, lat 22.575~22.582
        for &(la, lo) in &g.coords {
            assert!((108.232..=108.241).contains(&lo), "lng {lo}");
            assert!((22.575..=22.582).contains(&la), "lat {la}");
        }
    }

    /// 离线全链：广西职业技术大学打卡点必须能规划出沿校园道路的闭合环。
    /// 纯本地计算，无网络。
    #[test]
    fn test_plan_ring_guangxi_vocational_technical_university() {
        // POI 的 GCJ 坐标转 BD 后模拟服务端下发的打卡点
        let pts_bd: Vec<(f64, f64)> = POIS_GCJ
            .iter()
            .map(|&(_, lo, la)| {
                let (bla, blo) = gcj02_to_bd09(la, lo);
                (bla, blo)
            })
            .collect();
        let mut lines = Vec::new();
        let ring = plan_road_ring(&pts_bd, &mut |s: &str| lines.push(s.to_string()));
        for line in &lines {
            println!("{line}");
        }
        let ring = ring.expect("校内打卡点应能沿内置路网规划道路环");
        // 每个打卡点必须被环精确命中（坐标往返误差 < 2m）
        for (i, p) in pts_bd.iter().enumerate() {
            let d = ring.iter().map(|q| dist_m(*p, *q)).fold(f64::INFINITY, f64::min);
            assert!(d < 2.0, "打卡点 {i} 距环 {d:.1}m");
        }
        let mut total: f64 = ring.windows(2).map(|w| dist_m(w[0], w[1])).sum();
        if let (Some(first), Some(last)) = (ring.first(), ring.last()) {
            total += dist_m(*last, *first);
        }
        println!("环总长 {total:.0}m，平均段长 {:.1}m", total / ring.len() as f64);
        assert!((500.0..=6000.0).contains(&total), "环长 {total:.0}m 超出校园合理范围");
        assert!((total / ring.len() as f64) < 120.0, "平均段长过大，疑似未沿路网");
    }

    /// 非 GXVTU 的打卡点（如大连某校园）接不进内置路网，必须回退直线环。
    #[test]
    fn test_plan_ring_rejects_off_campus_points() {
        let pts = vec![
            (38.901678, 121.540241),
            (38.902564, 121.541233),
            (38.900921, 121.542310),
        ];
        let mut lines = Vec::new();
        let ring = plan_road_ring(&pts, &mut |s: &str| lines.push(s.to_string()));
        assert!(ring.is_none(), "校外打卡点不应接入校园路网");
        assert!(lines.iter().any(|l| l.contains("回退直线环")));
    }

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
}
