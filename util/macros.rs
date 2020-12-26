// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

#[macro_export]
macro_rules! make_err {
    ($($arg:tt)+) => {{
        Error::new(
            ErrorKind::InvalidInput,
            format!("{}", format_args!($($arg)+)
            ),
        )
    }};
}

#[macro_export]
macro_rules! error_if {
    ($cond:expr, $($arg:tt)+) => {{
      if $cond {
        return Err(make_err!($($arg)+));
      }
    }}
}
