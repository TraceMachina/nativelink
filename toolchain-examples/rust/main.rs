fn main() {
    println!("Hello, World!");
}

fn get_greeting() -> String {
    String::from("Hello, World!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_greeting() {
        assert_eq!(get_greeting(), "Hello, World!");
    }
}
