# ARB — карта покрытия DEX на BSC

Копия документа проекта claude.ai `claude/dex-coverage-map.md` (актуально на 31.10–01.11.2026). Полная версия с адресами
фабрик, роутеров и квотеров, с пометками [on-chain ✓], — в документе проекта; перенос сюда — по правилу синхронизации
(CLAUDE.md, 02.11.2026).

## Итог
- ~94–95% объёма BSC уже покрыто (Pancake, Uniswap, THENA Fusion, DODO, V2/V3-форки, Lista-стейблсвап).
- Плюс один модуль кода (Solidly-стейбл) и проверка конфига Slipstream → ~97–98% (входит Topaz).
- ~3% — другой тип (PropAMM / RFQ / оракул / книга ордеров: Tessera V, Native, Metric, ElfomoFi, Textile FX, Dexalot,
  Lunarbase): нет читаемых резервов, это отдельный «источник котировок», часто закрытый API.

## Нужно кодом (приоритет = объём)
1. Solidly-стейбл-инвариант `x³y + xy³ = k`: Topaz v2-stable и THENA V1 stable.
2. Topaz v3 Slipstream (CL по tick-spacing): проверить, ложится ли на CL-адаптер конфигом.
3. Maverick V2: своп-петля по бинам.

## Покрыто конфигом
V2/V3-форки (решения 59, 82): Biswap, Sushi, BabyDogeSwap, BabySwap, ApeSwap, Nomiswap, MDEX, BakerySwap, Forest, Unchain X.
Стейблсвапы (Curve-math): Lista DAO, Nerve, Ellipsis. Volatile-пулы Topaz v2 и THENA V1.

## Включены на сервере (01.11.2026)
SushiSwap V2, PancakeSwap V1, ApeSwap, Nomiswap, Biswap, BabySwap, MDEX, BaryonSwap (только сигналы — роутер не найден),
THENA Fusion, KyberSwap Elastic, THENA V1, Topaz (Slipstream). DODO: индексатор пулы на BSC не отдаёт.
Адреса — в настройках `dex.*` (не в коде), полный перечень — в документе проекта.

## Перепроверить
Стандартность Curve-инварианта у Lista; Unchain X и Forest — обычные Uni-V3/V2; разбор Slipstream (tick-spacing + фабрика).
