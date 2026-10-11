<?php

declare(strict_types=1);

namespace Billing\Tests;

use Billing\Invoice;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

final class InvoiceTest extends TestCase
{
    public static function rates(): array
    {
        return [
            'zero' => [100, 0, 100],
            'standard' => [100, 19, 119],
        ];
    }

    #[DataProvider('rates')]
    public function testTotal(int $net, int $rate, int $total): void
    {
        $this->assertSame($total, (new Invoice())->total($net, $rate));
    }

    public function testFailsInPackage(): void
    {
        $this->assertSame(1, (new Invoice())->refund(1));
    }
}
