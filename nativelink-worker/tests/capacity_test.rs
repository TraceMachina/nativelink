use nativelink_worker::capacity::{
    free_memory_kb_from, parse_meminfo_available_kb, parse_memory_current_kb,
};
use pretty_assertions::assert_eq;

/// The keepalive reports the limit less current usage when there is a
/// limit, and what the host has available when there is not.
#[test]
fn free_memory_is_limit_less_current_or_the_host_available() {
    assert_eq!(
        parse_memory_current_kb("10737418240\n"),
        Some(10 * 1024 * 1024)
    );
    assert_eq!(parse_memory_current_kb("max"), None);
    assert_eq!(
        parse_meminfo_available_kb("MemTotal: 65536000 kB\nMemAvailable:   12345678 kB\n"),
        Some(12_345_678)
    );
    assert_eq!(
        free_memory_kb_from(Some(52 * 1024 * 1024), Some(40 * 1024 * 1024), Some(1)),
        Some(12 * 1024 * 1024)
    );
    assert_eq!(free_memory_kb_from(Some(100), Some(200), Some(1)), Some(0));
    assert_eq!(free_memory_kb_from(None, Some(200), Some(777)), Some(777));
    assert_eq!(free_memory_kb_from(None, None, None), None);
}
