//! 轨迹海拔覆盖。
//!
//! 海拔是轨迹点的一部分，而不是提交时临时拼出来的字段。统一在生成器
//! 完成后覆盖 `bdA`，这样提交体的 totalAscent、OBS 的 run_data 和每圈
//! elevationGain 都会读取同一份数据。

#![allow(non_snake_case)]

use super::geom::round_to;
use super::model::Track;

/// 将轨迹所有点的百度海拔覆盖为用户输入的绝对海拔（米）。
///
/// 返回 `Err` 而不是静默接受 NaN/无穷值，避免生成不可序列化或被服务端
/// 拒绝的提交。海拔范围采用常见地表范围，既能覆盖地下场地也能覆盖高原。
pub fn override_bd_a(track: &mut Track, altitude_m: f64) -> Result<(), String> {
    if !altitude_m.is_finite() {
        return Err("手动海拔必须是有限数字".into());
    }
    if !(-500.0..=9000.0).contains(&altitude_m) {
        return Err("手动海拔必须在 -500 到 9000 米之间".into());
    }
    let altitude_m = round_to(altitude_m, 2);
    for point in &mut track.locations {
        point.bdA = altitude_m;
        point.hasAltitude = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::submit::total_ascent;
    use crate::track::generator::build;

    fn points() -> Vec<(f64, f64)> {
        vec![(38.901678, 121.540241), (38.902564, 121.541233)]
    }

    #[test]
    fn override_replaces_every_bd_a_and_clears_ascent() {
        let mut track = build(1200.0, 600, 7, (38.9, 121.54), 1_700_000_000_000, &points());
        override_bd_a(&mut track, 36.75).unwrap();
        assert!(track.locations.iter().all(|p| p.bdA == 36.75));
        assert_eq!(total_ascent(&track.locations), 0.0);
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_values() {
        let mut track = build(1000.0, 500, 1, (38.9, 121.54), 1_700_000_000_000, &points());
        assert!(override_bd_a(&mut track, f64::NAN).is_err());
        assert!(override_bd_a(&mut track, 9001.0).is_err());
    }
}
