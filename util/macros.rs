// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

#[macro_export]
macro_rules! make_err_with_code {
    ($code:expr, $($arg:tt)+) => {{
        use tokio::io::Error;
        Error::new(
            $code,
            format!("{}", format_args!($($arg)+)),
        )
    }};
}

#[macro_export]
macro_rules! make_err {
    ($($arg:tt)+) => {{
        $crate::make_err_with_code!(tokio::io::ErrorKind::InvalidInput, $($arg)+)
    }};
}

#[macro_export]
macro_rules! error_if {
    ($cond:expr, $($arg:tt)+) => {{
      if $cond {
        Err($crate::make_err!($($arg)+))?;
      }
    }}
}
