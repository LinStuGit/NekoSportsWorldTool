//! 轨迹几何与随机工具。
//!
//! round 封装、RNG、打卡点线段环 + 折线环 + 弧长表 + 弧长插值。

use chrono::{Local, TimeZone};
use rand::rngs::StdRng;
use rand::SeedableRng;
use rand_distr::{Distribution, Normal};

pub const MET_PER_DEG_LAT: f64 = 111_132.0;
pub const MET_PER_DEG_LNG: f64 = 86_600.0;

/// round(x, n)：按精确二进制值四舍五入。
pub fn round_to(x: f64, n: usize) -> f64 {
    let s = format!("{x:.n$}");
    s.parse().unwrap_or(x)
}

/// 可播种 RNG 封装。
pub struct Rng {
    inner: StdRng,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { inner: StdRng::seed_from_u64(seed) }
    }
    pub fn random(&mut self) -> f64 {
        rand::Rng::gen_range(&mut self.inner, 0.0..1.0)
    }
    pub fn uniform(&mut self, a: f64, b: f64) -> f64 {
        rand::Rng::gen_range(&mut self.inner, a..=b)
    }
    pub fn randint(&mut self, a: i64, b: i64) -> i64 {
        rand::Rng::gen_range(&mut self.inner, a..=b)
    }
    pub fn gauss(&mut self, mu: f64, sigma: f64) -> f64 {
        Normal::new(mu, sigma).unwrap().sample(&mut self.inner)
    }
    pub fn choice<T>(&mut self, items: &[T]) -> T
    where
        T: Copy,
    {
        items[rand::Rng::gen_range(&mut self.inner, 0..items.len())]
    }
    pub fn weighted<T>(&mut self, items: &[(T, u32)]) -> T
    where
        T: Copy,
    {
        let total: u32 = items.iter().map(|(_, w)| *w).sum();
        let mut u = self.uniform(0.0, total as f64);
        for (item, w) in items {
            u -= *w as f64;
            if u < 0.0 {
                return *item;
            }
        }
        items[items.len() - 1].0
    }
}

/// 打卡点 BD 系 (lat, lng) → 闭合平面路径 + 弧长表 + 中心。
pub type PointRing = (Vec<(f64, f64)>, Vec<f64>, (f64, f64));
pub fn make_point_ring(bd_points: &[(f64, f64)]) -> PointRing {
    let n = bd_points.len();
    let cx = bd_points.iter().map(|q| q.0).sum::<f64>() / n as f64;
    let cy = bd_points.iter().map(|q| q.1).sum::<f64>() / n as f64;
    let mut ordered = bd_points.to_vec();
    ordered.sort_by(|a, b| {
        let ka = (a.0 - cx).atan2(a.1 - cy);
        let kb = (b.0 - cx).atan2(b.1 - cy);
        ka.partial_cmp(&kb).unwrap()
    });
    let plane: Vec<(f64, f64)> = ordered
        .iter()
        .map(|q| ((q.1 - cy) * MET_PER_DEG_LNG, (q.0 - cx) * MET_PER_DEG_LAT))
        .collect();
    make_polyline_ring(plane, (cx, cy))
}

/// 任意闭合折线（BD 系，已按行进顺序）→ 平面稠密环 + 弧长表 + 中心。
///
/// 路网规划得到的道路环直接使用本函数；与 [make_point_ring] 的区别是
/// 不再做极角排序（顺序由路网最短路给出），也不做直线插值。
pub fn make_polyline_ring(bd_polyline: Vec<(f64, f64)>, center: (f64, f64)) -> PointRing {
    let (cx, cy) = center;
    let dense: Vec<(f64, f64)> = bd_polyline
        .iter()
        .map(|q| ((q.1 - cy) * MET_PER_DEG_LNG, (q.0 - cx) * MET_PER_DEG_LAT))
        .collect();
    let mut arcs = vec![0.0f64];
    for i in 1..=dense.len() {
        let a = dense[i - 1];
        let b = dense[i % dense.len()];
        arcs.push(arcs[i - 1] + ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt());
    }
    (dense, arcs, (cx, cy))
}

/// 环线弧长 → 坐标（线性插值）。
pub fn ring_point_at(dense: &[(f64, f64)], arcs: &[f64], s: f64) -> (f64, f64) {
    let total = *arcs.last().unwrap_or(&1.0);
    let s = s.rem_euclid(total);
    let mut lo = 0usize;
    let mut hi = arcs.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if arcs[mid] < s {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let i = lo.max(1);
    let a = dense[(i - 1) % dense.len()];
    let b = dense[i % dense.len()];
    let seg = arcs[i] - arcs[i - 1];
    let t = if seg > 0.0 { (s - arcs[i - 1]) / seg } else { 0.0 };
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

pub fn to_bd(x: f64, y: f64, c_lat: f64, c_lng: f64) -> (f64, f64) {
    (c_lat + y / MET_PER_DEG_LAT, c_lng + x / MET_PER_DEG_LNG)
}

pub fn fmt_gain_time(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

