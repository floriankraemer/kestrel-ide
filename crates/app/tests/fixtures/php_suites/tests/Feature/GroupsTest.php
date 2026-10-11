<?php

declare(strict_types=1);

namespace Tests\Feature;

use PHPUnit\Framework\Attributes\Group;
use PHPUnit\Framework\TestCase;

final class GroupsTest extends TestCase
{
    #[Group('slow')]
    public function testSlow(): void
    {
        $this->assertTrue(true);
    }

    #[Group('db')]
    public function testDb(): void
    {
        $this->assertTrue(true);
    }

    public function testPlain(): void
    {
        $this->assertTrue(true);
    }
}
