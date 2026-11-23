// Старые полные подборы включали диапазон «допустимо» между причиной выбора и
// пояснениями исполнителя. Заменяем только узнаваемый фрагмент из profit::why.
const oldRange = /оптимум ([^;]+): прибыль ([^;]+); (?:на бюджете ([^; ]+) \([^;]+\) — ([^;]+)|это и есть наибольший объём в пределах бюджета ([^;]+)); допустимо [^;–]+–([^;]+?), посчитано объёмов /u;

export function explainSize(text) {
  if (!text?.includes('; допустимо ')) return text;
  const result = text.replace(oldRange, (_, chosen, profit, budget, edgeProfit, fullBudget, edge) =>
    `Подбор выбрал ${chosen}, прибыль ${profit} USDT при бюджете ${budget ?? fullBudget} USDT. ` +
    `Граница участка поиска — ${edge}, прибыль ${edgeProfit ?? profit} USDT. ` +
    'Общий максимум объёма не установлен. Посчитано объёмов ');
  return result === text ? text : result.replace('граница бюджета не уточнена до шага (мало size.bisect_steps)',
    'граница участка не уточнена до шага');
}
