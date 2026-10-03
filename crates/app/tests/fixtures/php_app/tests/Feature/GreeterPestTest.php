<?php

declare(strict_types=1);

use App\Greeter;

it('greets by name', function () {
    expect((new Greeter('Hi'))->greet('Pest'))->toBe('Hi, Pest!');
});

test('whispers in lower case', function () {
    expect((new Greeter())->whisper('Pest'))->toBe('hello, pest!');
});
