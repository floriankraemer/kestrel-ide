<?php

declare(strict_types=1);

namespace App;

final class Mailer
{
    public function send(string $to): string
    {
        return 'sent to ' . $to;
    }

    public function bounce(string $to): string
    {
        return 'bounced ' . $to;
    }
}
