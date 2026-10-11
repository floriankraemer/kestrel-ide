<?php

declare(strict_types=1);

namespace Tests\Unit;

use PHPUnit\Framework\TestCase;

final class OutcomesTest extends TestCase
{
    public function testPasses(): void
    {
        $value = 40 + 2;
        $this->assertSame(42, $value);
    }

    public function testFails(): void
    {
        $this->assertSame('expected text', 'actual text');
    }

    public function testSkipped(): void
    {
        $this->markTestSkipped('not on this platform');
    }

    public function testErrors(): void
    {
        throw new \RuntimeException('boom');
    }
}
