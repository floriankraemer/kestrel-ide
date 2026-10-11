<?php

declare(strict_types=1);

namespace Tests\Unit;

use App\Mailer;
use PHPUnit\Framework\TestCase;

final class MailerTest extends TestCase
{
    public function testSend(): void
    {
        $this->assertSame('sent to ann', (new Mailer())->send('ann'));
    }
}
