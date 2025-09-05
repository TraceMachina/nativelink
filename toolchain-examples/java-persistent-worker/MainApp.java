package com.example;

/**
 * Main application demonstrating use of Calculator and StringUtils.
 */
public class MainApp {
    public static void main(String[] args) {
        Calculator calc = new Calculator();

        System.out.println("=== Calculator Demo ===");
        System.out.println("2 + 3 = " + calc.add(2, 3));
        System.out.println("10 - 4 = " + calc.subtract(10, 4));
        System.out.println("5 * 6 = " + calc.multiply(5, 6));
        System.out.println("15 / 3 = " + calc.divide(15, 3));
        System.out.println("2^8 = " + calc.power(2, 8));
        System.out.println("sqrt(16) = " + calc.sqrt(16));

        System.out.println("\n=== StringUtils Demo ===");
        System.out.println("Reverse 'hello': " + StringUtils.reverse("hello"));
        System.out.println("Is 'racecar' palindrome? " + StringUtils.isPalindrome("racecar"));
        System.out.println("Capitalize 'java': " + StringUtils.capitalize("java"));
        System.out.println("Title case 'hello world': " + StringUtils.toTitleCase("hello world"));
        System.out.println("Word count 'The quick brown fox': " + StringUtils.countWords("The quick brown fox"));
        System.out.println("Repeat 'Hi! ' 3 times: " + StringUtils.repeat("Hi! ", 3));

        System.out.println("\nDemo completed successfully!");
    }
}
