<?php

declare(strict_types=1);

require __DIR__ . '/../vendor/autoload.php';

$name = $argv[1] ?? 'World';
$greeter = new App\Greeter();
$message = $greeter->greet($name);
echo $message, PHP_EOL;
