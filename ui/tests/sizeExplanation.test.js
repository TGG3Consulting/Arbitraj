import assert from 'node:assert/strict';
import { test } from 'node:test';
import { explainSize } from '../src/lib/sizeExplanation.js';

// Сохранённая строка plan.size.why сделки 20261118213857378SIREN, MEXC → BSC.
const siren = 'оптимум 2936.62 SIREN: прибыль 0.1673; на бюджете 96.3215 (46.11 SIREN) — -0.076; допустимо 40.66–46.11 SIREN, посчитано объёмов 68, пиков 1';

test('SIREN shows the chosen quantity and the first search boundary separately', () => {
  assert.equal(explainSize(siren), 'Подбор выбрал 2936.62 SIREN, прибыль 0.1673 USDT при бюджете 96.3215 USDT. Граница участка поиска — 46.11 SIREN, прибыль -0.076 USDT. Общий максимум объёма не установлен. Посчитано объёмов 68, пиков 1');
});

test('keeps simulation limits, threshold retries and the final resize visible', () => {
  const base = 'оптимум 402 ACE: прибыль 19.2122; это и есть наибольший объём в пределах бюджета 7.3989; допустимо 4–402 ACE, посчитано объёмов 55, пиков 1';
  const resize = '; по финальной котировке объём ужат (бюджет или глубина исполнителя): 203 ACE';
  const prefix = 'объём ограничен симуляцией продажи до покупки (решение 380); подбор на проверяемом объёме: ';
  const suffix = '; поиск остановлен пределом size.max_evals — лучший из посчитанных; по финальной котировке на 203 ACE прибыль в % ниже порога 2 — взят ближайший меньший проходящий оба порога объём 193 ACE';
  const text = explainSize(prefix + base + resize + suffix);
  assert.ok(text.startsWith(prefix + 'Подбор выбрал 402 ACE'));
  assert.ok(text.includes('Граница участка поиска — 402 ACE, прибыль 19.2122 USDT'));
  assert.ok(text.endsWith(resize + suffix));
  assert.ok(explainSize(`ниже порога (${siren})`).startsWith('ниже порога (Подбор выбрал'));
});

test('one-point, cycle, new and unrecognized archive formats pass through verbatim', () => {
  for (const text of [undefined, '', 'объём кандидата 5 CAKE пересчитан на свежих данных: прибыль 1; повторный подбор объёма не выполнялся (precheck.fast_recalc)',
    'наибольшая чистая прибыль цикла 1 USDT на входе 10 (бюджет 20 USDT)',
    explainSize(siren), 'иной формат; допустимо 1–2 CAKE, посчитано объёмов 3']) {
    assert.equal(explainSize(text), text);
  }
});
