<?php

declare(strict_types=1);

namespace Tests\Extra;

use PHPUnit\Framework\TestCase;

// Outside every suite: a run long enough to stop, or to start another over.
final class SlowTest extends TestCase
{
    public function testWaits(): void
    {
        sleep(30);
        $this->assertTrue(true);
    }
}
