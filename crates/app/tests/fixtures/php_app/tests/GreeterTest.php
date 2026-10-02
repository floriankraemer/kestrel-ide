<?php

declare(strict_types=1);

namespace Tests;

use App\Greeter;
use PHPUnit\Framework\TestCase;

final class GreeterTest extends TestCase
{
    public function testGreetsByName(): void
    {
        $greeter = new Greeter();
        $message = $greeter->greet('World');
        $this->assertSame('Hello, World!', $message);
    }

    public function testWhispersInLowerCase(): void
    {
        $this->assertSame('hello, world!', (new Greeter())->whisper('World'));
    }
}
