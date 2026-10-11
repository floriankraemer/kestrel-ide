<?php

declare(strict_types=1);

describe('outer', function () {
    describe('inner', function () {
        it('adds', function () {
            expect(1 + 1)->toBe(2);
        });

        it('subtracts', function () {
            expect(2 - 1)->toBe(1);
        });
    });

    it('squares', function (int $n, int $square) {
        expect($n * $n)->toBe($square);
    })->with([
        'two' => [2, 4],
        'three' => [3, 9],
    ]);
});
