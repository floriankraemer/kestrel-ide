<?php

declare(strict_types=1);

namespace App;

interface GreeterInterface
{
    public function greet(string $name): string;
}
