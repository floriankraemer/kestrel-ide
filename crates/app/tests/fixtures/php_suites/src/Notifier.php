<?php

declare(strict_types=1);

namespace App;

final class Notifier
{
    public function __construct(private Mailer $mailer)
    {
    }

    public function notify(string $to): string
    {
        return $this->mailer->send($to);
    }
}
