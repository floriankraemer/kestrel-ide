<?php

declare(strict_types=1);

namespace Tests\Unit;

use App\Mailer;
use App\Notifier;
use PHPUnit\Framework\Attributes\CoversClass;
use PHPUnit\Framework\TestCase;

#[CoversClass(Mailer::class)]
final class NotifierTest extends TestCase
{
    public function testNotify(): void
    {
        $this->assertSame('sent to bob', (new Notifier(new Mailer()))->notify('bob'));
    }
}
