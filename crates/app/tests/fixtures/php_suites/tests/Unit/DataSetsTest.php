<?php

declare(strict_types=1);

namespace Tests\Unit;

use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

final class DataSetsTest extends TestCase
{
    public static function named(): array
    {
        return [
            'a/b' => ['a/b'],
            'x@y' => ['x@y'],
            'plain' => ['plain'],
        ];
    }

    public static function unnamed(): array
    {
        return [[1], [2]];
    }

    #[DataProvider('named')]
    public function testNamed(string $value): void
    {
        $this->assertNotSame('', $value);
    }

    #[DataProvider('unnamed')]
    public function testUnnamed(int $value): void
    {
        $this->assertGreaterThan(0, $value);
    }
}
