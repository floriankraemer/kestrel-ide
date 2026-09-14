package com.example;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

class CalcTest {

    @Test
    void addsTwoNumbers() {
        assertEquals(4, new Calc().add(2, 2));
    }
}
