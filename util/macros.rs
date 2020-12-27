// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

#[macro_export]
macro_rules! make_err {
    ($code:expr, $($arg:tt)+) => {{
        tokio::io::Error::new(
            $code,
            format!("{}", format_args!($($arg)+)),
        )
    }};
}

#[macro_export]
macro_rules! make_input_err {
    ($($arg:tt)+) => {{
        $crate::make_err!(tokio::io::ErrorKind::InvalidInput, $($arg)+)
    }};
}

#[macro_export]
macro_rules! error_if {
    ($cond:expr, $err:expr) => {{
        if $cond {
            Err($err)?;
        }
    }};
}
