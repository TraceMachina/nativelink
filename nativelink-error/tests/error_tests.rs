use nativelink_error::Error;
use walkdir::WalkDir;

#[test]
fn walkdir_source_error() {
    for entry in WalkDir::new("/bad/path") {
        let err: Error = entry.unwrap_err().into();
        let os_error = {
            #[cfg(unix)]
            {
                "No such file or directory (os error 2)"
            }
            #[cfg(windows)]
            {
                "The system cannot find the path specified. (os error 3)"
            }
        };
        assert_eq!(
            err.messages,
            vec![
                os_error,
                &format!("IO error for operation on /bad/path: {os_error}")
            ]
        );
    }
}
