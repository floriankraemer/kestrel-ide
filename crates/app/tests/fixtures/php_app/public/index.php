<?php

declare(strict_types=1);

require __DIR__ . '/../vendor/autoload.php';

$greeter = new App\Greeter();
$name = $_GET['name'] ?? 'World';
$message = $greeter->greet($name);
echo $message;
