//! 显示用的小格式化:字节数。文件管理器与预览窗口共用。

/// 字节数 → 人看的大小(1024 进制,保留一位小数)。
pub(crate) fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        // 字节数不带小数:`1 B` 比 `1.0 B` 顺眼。
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn size_switches_units_at_kib() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1024), "1.0 KiB");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(1024 * 1024 * 3 / 2), "1.5 MiB");
    }
}
