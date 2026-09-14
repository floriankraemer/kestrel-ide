package com.example;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

class GreeterTest {

    @Test
    void greetsByName() {
        assertEquals("Hello, World!", new Greeter().greet("World"));
    }

    @Test
    void deliberatelyFails() {
        assertEquals("Hello, World!", new Greeter().greet("Nobody"));
    }
}
