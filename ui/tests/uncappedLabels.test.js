import assert from 'node:assert/strict';
import { test } from 'node:test';
import { limitedLabel } from '../src/lib/uncappedLabels.js';

test('a capped model reports the best checked volume within budget', () => {
  assert.equal(limitedLabel('optimum', true), 'лучший проверенный объём укладывается в бюджет (достигнут предел расчёта)');
  assert.equal(limitedLabel('optimum'), 'лучший проверенный объём укладывается в бюджет');
  assert.equal(limitedLabel('free', true), limitedLabel('free'));
});
