package com.example.app;

import com.example.lib.Greeting;

public class App {
    public String run(String name) {
        return Greeting.of(name);
    }
}
