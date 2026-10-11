<?php

declare(strict_types=1);

namespace Billing;

final class Invoice
{
    public function total(int $net, int $taxPercent): int
    {
        return $net + intdiv($net * $taxPercent, 100);
    }

    public function refund(int $amount): int
    {
        return -$amount;
    }
}
