<?php

declare(strict_types=1);

namespace App;

final class Greeter implements GreeterInterface
{
    public function __construct(private readonly string $greeting = 'Hello')
    {
    }

    public function greet(string $name): string
    {
        return sprintf('%s, %s!', $this->greeting, $name);
    }

    // Planted PHPStan error: declared int, returns string.
    public function shout(string $name): int
    {
        return strtoupper($this->greet($name));
    }

    // Planted PSR-12 violations: no space before the return type, bad indent.
    public function whisper(string $name):string
    {
      return strtolower($this->greet($name));
    }
}
