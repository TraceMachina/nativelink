package com.example;

import org.junit.Test;
import org.junit.Before;
import static org.junit.Assert.*;

public class CalculatorTest {
    private Calculator calculator;

    @Before
    public void setUp() {
        calculator = new Calculator();
    }

    @Test
    public void testAddition() {
        assertEquals(5, calculator.add(2, 3));
        assertEquals(0, calculator.add(-5, 5));
        assertEquals(-10, calculator.add(-5, -5));
    }

    @Test
    public void testSubtraction() {
        assertEquals(-1, calculator.subtract(2, 3));
        assertEquals(10, calculator.subtract(15, 5));
        assertEquals(0, calculator.subtract(5, 5));
    }

    @Test
    public void testMultiplication() {
        assertEquals(6, calculator.multiply(2, 3));
        assertEquals(0, calculator.multiply(0, 100));
        assertEquals(-15, calculator.multiply(3, -5));
    }

    @Test
    public void testDivision() {
        assertEquals(2, calculator.divide(6, 3));
        assertEquals(-3, calculator.divide(9, -3));
        assertEquals(0, calculator.divide(0, 5));
    }

    @Test(expected = IllegalArgumentException.class)
    public void testDivisionByZero() {
        calculator.divide(10, 0);
    }

    @Test
    public void testPower() {
        assertEquals(8.0, calculator.power(2, 3), 0.001);
        assertEquals(1.0, calculator.power(5, 0), 0.001);
        assertEquals(0.25, calculator.power(2, -2), 0.001);
    }

    @Test
    public void testSquareRoot() {
        assertEquals(3.0, calculator.sqrt(9), 0.001);
        assertEquals(5.0, calculator.sqrt(25), 0.001);
        assertEquals(0.0, calculator.sqrt(0), 0.001);
    }

    @Test(expected = IllegalArgumentException.class)
    public void testNegativeSquareRoot() {
        calculator.sqrt(-4);
    }
}
