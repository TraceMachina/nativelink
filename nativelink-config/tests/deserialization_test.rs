use nativelink_config::serde_utils::{
    convert_data_size_with_shellexpand, convert_duration_with_shellexpand,
};

#[cfg(test)]
mod tests {
    use serde::de::value::{Error as ValueError, I64Deserializer, StrDeserializer};
    use serde::de::IntoDeserializer;

    use super::*;

    #[test]
    fn test_duration_numeric() {
        let deserializer: I64Deserializer<ValueError> = 10i64.into_deserializer();
        assert_eq!(convert_duration_with_shellexpand(deserializer), Ok(10));
    }

    #[test]
    fn test_duration_seconds_string() {
        let deserializer: StrDeserializer<ValueError> = "1s".into_deserializer();
        assert_eq!(convert_duration_with_shellexpand(deserializer), Ok(1));
    }

    #[test]
    fn test_duration_minutes_string() {
        let deserializer: StrDeserializer<ValueError> = "1m".into_deserializer();
        assert_eq!(convert_duration_with_shellexpand(deserializer), Ok(60));
    }

    #[test]
    fn test_duration_minutes_seconds_string() {
        let deserializer: StrDeserializer<ValueError> = "1m 1s".into_deserializer();
        assert_eq!(convert_duration_with_shellexpand(deserializer), Ok(61));
    }

    #[test]
    fn test_data_size_numeric() {
        let deserializer: I64Deserializer<ValueError> = 10i64.into_deserializer();
        assert_eq!(convert_data_size_with_shellexpand(deserializer), Ok(10));
    }

    #[test]
    fn test_data_size_binary_multiples() {
        let deserializer: StrDeserializer<ValueError> = "1KiB".into_deserializer();
        assert_eq!(convert_data_size_with_shellexpand(deserializer), Ok(1024));
    }

    #[test]
    fn test_data_size_traditional_multiples() {
        let deserializer: StrDeserializer<ValueError> = "1KB".into_deserializer();
        assert_eq!(convert_data_size_with_shellexpand(deserializer), Ok(1000));
    }
}
