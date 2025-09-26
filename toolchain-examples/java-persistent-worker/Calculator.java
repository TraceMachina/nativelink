package com.example;

/**
 * Simple calculator class to demonstrate persistent worker compilation.
 */
public class Calculator {

    public int add(int a, int b) {
        // Using persistent workers for faster compilation!
        return a + b;
    }

    public int subtract(int a, int b) {
        return a - b;
    }

    public int multiply(int a, int b) {
        return a * b;
    }

    public int divide(int a, int b) {
        if (b == 0) {
            throw new IllegalArgumentException("Cannot divide by zero");
        }
        return a / b;
    }

    public double power(double base, double exponent) {
        return Math.pow(base, exponent);
    }

    public double sqrt(double value) {
        if (value < 0) {
            throw new IllegalArgumentException("Cannot calculate square root of negative number");
        }
        return Math.sqrt(value);
    }
}
