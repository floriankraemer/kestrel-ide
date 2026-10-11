<?php

declare(strict_types=1);

namespace Util\Tests;

use PHPUnit\Framework\TestCase;
use Util\Slug;

final class SlugTest extends TestCase
{
    public function testSlugs(): void
    {
        $this->assertSame('a-b', Slug::of('A B'));
    }
}
