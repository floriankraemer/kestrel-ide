<?php

declare(strict_types=1);

namespace Tests\Extra;

use PHPUnit\Framework\TestCase;

// Outside every suite: passes only with the configuration's environment.
final class EnvTest extends TestCase
{
    public function testAppEnv(): void
    {
        $this->assertSame('test', getenv('APP_ENV'));
    }
}
