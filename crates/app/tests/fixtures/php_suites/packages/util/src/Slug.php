<?php

declare(strict_types=1);

namespace Util;

final class Slug
{
    public static function of(string $text): string
    {
        return strtolower(str_replace(' ', '-', $text));
    }
}
