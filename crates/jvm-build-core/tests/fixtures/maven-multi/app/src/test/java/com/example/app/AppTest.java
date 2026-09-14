package com.example.app;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

class AppTest {

    @Test
    void runsThroughTheLibDependency() {
        assertEquals("Hello, World!", new App().run("World"));
    }
}
