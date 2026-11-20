// Файл: contracts/evmtest/ekubo_twamm.mjs
// Что делает: решение 458, T458-02 — шаг EKUBO (25) исполнителя ArbExecutor v4 на пуле с расширением TWAMM, ОФИЦИАЛЬНЫЙ
//   код из файлов развёртывания Arbitrum (vendor/ekubo/broadcast, EkuboProtocol/evm-contracts):
//   TWAMM 0xd4f1…ed9b — DeployAll.s.sol/42161/run-1767194711580.json (коммит ccb3a419, сборка 61d09da; исходник
//   src/extensions/TWAMM.sol @61d09daa), менеджер ордеров Orders 0xff6c…0644 (аргумент — этот TWAMM) —
//   DeployManagers.s.sol/42161/run-1767205373297.json (коммит 504b42f6, сборка 8a99714; src/Orders.sol); второй TWAMM сети
//   0xd47f…75ba — DeployAll.s.sol/42161/run-1777073880286.json (коммит 941eb76a, сборка 8c1b721) с Orders 0x3325…98b4 —
//   DeployManagers.s.sol/42161/run-1777130801606.json (коммит 20c92e22, сборка 34bbea9); сценарий T1–T3 — на обоих. Развёртывание —
//   тем же CREATE2-прокси, что ekubo.mjs (адреса = адресам в сети).
//   Пул TWAMM — полный диапазон (TWAMM.sol L633-L651 afterInitializePool: FullRangePoolOnly), ликвидность — Positions
//   на [MIN_TICK, MAX_TICK]; ордер продажи token0 заводит LP официальным Orders.mintAndIncreaseSellAmount (Orders.sol
//   L43-L51; startTime 0 — уже идёт, endTime — допустимое время math/time.sol L17-L40), затем время блока сдвигается на час.
//   T1) обмен через пул с активным ордером: перед обменом ядро зовёт TWAMM.beforeSwap → lockAndExecuteVirtualOrders
//       (TWAMM.sol L655-L658, L613-L630) — в той же транзакции событие VirtualOrdersExecuted расширения (log0 60 байт,
//       L70-L82) и обмен ядра виртуальных ордеров до block.timestamp, затем обмен исполнителя; получено = котировка
//       ядра (fixtures/EkuboQuoter.sol — тот же путь, то же время блока, исполнение ордеров включено), списано amountIn,
//       на исполнителе пусто; minOut + 1 — «minOut»; обе стороны;
//   T2) второй обмен в том же блоке — ордера уже исполнены до block.timestamp: одна запись обмена ядра, получено = котировка;
//   T3) forward = true к TWAMM — откат: handleForwardData TWAMM читает первое слово как callType (0 — ордер, 1 —
//       выручка), у шага это token0 ключа (ERC-20, не 0 / 1) → revert() без данных (TWAMM.sol L189-L391); владелец при своих.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: ekubo.mjs, lib.mjs, world.mjs, vendor/ekubo.
// Настройки: нет.

import { Interface } from 'ethers';
import * as L from './lib.mjs';
import * as W from './world.mjs';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import * as K from './ekubo.mjs';

const { check, must } = L;
const { E18, OWNER, LP, ERC20, exe, bal } = W;
const { step, quote, coreSwaps, empty } = K;
const MAX_TICK = 88722835n; // math/constants.sol L14 @61d09daa
const FEE = ((1n << 64n) * 3n) / 1000n;
const ORD = new Interface(['function mintAndIncreaseSellAmount(tuple(address,address,bytes32) orderKey, uint112 amount, uint112 maxSaleRate) payable returns (uint256, uint112)']);
const hex = (v) => '0x' + v.toString(16).padStart(64, '0');
/// Записи VirtualOrdersExecuted расширения (log0 60 байт: poolId 32 | saleRate0 14 | saleRate1 14).
const voe = (r, twamm) => r.logs.filter((l) => l.address.toLowerCase() === twamm.toLowerCase() && l.topics.length === 0
  && (l.data.length - 2) / 2 === 60);

const bc = (p) => JSON.parse(readFileSync(join(L.here, 'vendor', 'ekubo', 'broadcast', p), 'utf8'));
// Два TWAMM сети (настройки бота: src/settings/catalog/dex_ekubo.rs, SETTINGS_CASES.md): у каждого свой Orders, ядро общее.
const VARIANTS = [
  { name: 'TWAMM 0xd4f1 (сборка 61d09da)', all: K.ALL, man: K.MAN },
  // DeployAll.s.sol/42161/run-1777073880286.json (коммит 941eb76a, сборка 8c1b721) — TWAMM 0xd47f…75ba;
  // DeployManagers.s.sol/42161/run-1777130801606.json (коммит 20c92e22, сборка 34bbea9) — Orders 0x3325…98b4.
  { name: 'TWAMM 0xd47f (сборка 8c1b721)', all: bc('DeployAll.s.sol/42161/run-1777073880286.json'), man: bc('DeployManagers.s.sol/42161/run-1777130801606.json') },
];

/// Мир ekubo.mjs + TWAMM и Orders варианта v, пул TWAMM USDT/COIN (полный диапазон) с ликвидностью и ордером продажи token0.
async function envTwamm(v) {
  const e = await K.env();
  const { c, usdt, coin } = e;
  const twamm = await K.create2(c, K.tx(v.all, 'TWAMM'));
  const orders = await K.create2(c, K.tx(v.man, 'Orders'));
  const [t0, t1] = K.sort(usdt, coin);
  const kT = [t0, t1, hex((BigInt(twamm) << 96n) | (FEE << 32n))]; // createFullRangePoolConfig (poolConfig.sol L175-L180)
  c.ts = 1_767_200_000n;
  await K.addLiquidity(e, kT, 0n, MAX_TICK);
  const end = (c.ts / 65536n + 2n) * 65536n; // кратно шагу 65536 (computeStepSize: разность < 2^20 → 2^16)
  const oc = hex((FEE << 192n) | end); // createOrderConfig(fee, isToken1 = false, start 0, end) — orderConfig.sol L52-L62
  await must(c, LP, t0, ERC20.abi, 'approve', [orders, 10n ** 30n]);
  const o = await L.call(c, LP, orders, ORD.encodeFunctionData('mintAndIncreaseSellAmount', [[t0, t1, oc], 2_000n * E18, (1n << 112n) - 1n]));
  if (!o.ok) throw new Error(`mintAndIncreaseSellAmount: ${o.reason} ${o.ret}`);
  c.ts += 3600n; // час виртуального исполнения ордера до обмена
  return { ...e, twamm, orders, kT, end };
}

export async function ekuboTwamm() {
  const record = { note: 'EVM ethereumjs Cancun + CLZ, contracts/out v4, официальные TWAMM / Orders (broadcast 42161) — contracts/evmtest/ekubo_twamm.mjs', cases: [] };
  for (const V of VARIANTS) {
    const e = await envTwamm(V);
    const { c, ex, core, twamm, kT, end } = e;
    const T = `EKUBO ${V.name}`;
    check(end > c.ts, `${T}: ордер продажи token0 активен (конец ${end}, время блока ${c.ts})`);
    for (const [tin, tout] of [[kT[0], kT[1]], [kT[1], kT[0]]]) {
      const dir = `${tin === e.usdt ? 'USDT' : 'COIN'} → ${tout === e.usdt ? 'USDT' : 'COIN'}`;
      const amt = 100n * E18;
      const [owe, q] = await quote(e, kT, tin, amt);
      check(owe === amt && q > 0n, `${T} ${dir}: котировка ядра с исполнением ордеров до block.timestamp — вход ${owe}, выход ${q}`);
      const s = step(core, tin, tout, kT[2]);
      const bad = await L.call(c, OWNER, ex, exe(tin, amt, tout, q + 1n, [s]));
      check(!bad.ok && bad.reason === 'minOut', `${T} ${dir}: minOut = котировка + 1 — откат «minOut»`);
      const ib = await bal(c, tin, OWNER);
      const ob = await bal(c, tout, OWNER);
      const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, q, [s]));
      check(r.ok, `${T} ${dir}: обмен прошёл (газ ${r.gas})`);
      const v = voe(r, twamm);
      check(v.length === 1 && BigInt('0x' + v[0].data.slice(66, 94)) > 0n && coreSwaps(r, core).length === 2,
        `${T} ${dir}: в транзакции исполнены виртуальные ордера (VirtualOrdersExecuted с saleRate0 > 0 + обмен ядра ордеров), затем обмен шага`);
      const got = (await bal(c, tout, OWNER)) - ob;
      check(got === q && ib - (await bal(c, tin, OWNER)) === amt, `${T} ${dir}: получено ${got} = котировка, списано ровно amountIn`);
      check(await empty(e, [tin, tout]), `${T} ${dir}: на исполнителе пусто`);
      const [, q2] = await quote(e, kT, tin, amt);
      const ob2 = await bal(c, tout, OWNER);
      const r2 = await L.call(c, OWNER, ex, exe(tin, amt, tout, q2, [s]));
      check(r2.ok && coreSwaps(r2, core).length === 1 && voe(r2, twamm).length === 0 && (await bal(c, tout, OWNER)) - ob2 === q2,
        `${T} ${dir}: второй обмен в том же блоке — ордера уже исполнены, одна запись обмена ядра, получено = котировка ${q2}`);
      record.cases.push({ kind: K.EKUBO, core, extension: twamm, token_in: tin, token_out: tout, data: s[5], amount_in: amt.toString(), got: got.toString(), gas: r.gas.toString() });
      c.ts += 600n; // следующая сторона — новый блок: снова есть что исполнять
    }
    const ub = await bal(c, kT[0], OWNER);
    // Откат без данных: TWAMM.handleForwardData — `revert()` (TWAMM.sol L389-L391); ядро (forward) и исполнитель
    // (_ekuboCall) пробрасывают его как есть. Что откатился именно вызов расширения (селектор 0x00000001 —
    // FlashAccountant.sol L200-L201) без данных — по ходу исполнения EVM (beforeMessage / afterMessage).
    const twammRev = [];
    const h = (r) => { if (r.execResult.exceptionError && r.execResult.returnValue.length === 0) twammRev.push(1); };
    const on = (m) => { if (m.to?.toString().toLowerCase() === twamm.toLowerCase() && m.data[0] === 0 && m.data[1] === 0 && m.data[2] === 0 && m.data[3] === 1) c.evm.events.once('afterMessage', h); };
    c.evm.events.on('beforeMessage', on);
    const f = await L.call(c, OWNER, ex, exe(kT[0], E18, kT[1], 0n, [step(core, kT[0], kT[1], kT[2], false, true)]));
    c.evm.events.off('beforeMessage', on);
    check(!f.ok && f.ret === '0x' && twammRev.length === 1 && (await bal(c, kT[0], OWNER)) === ub,
      `${T} с forward = true — откатился вызов расширения forwarded_2374103877 без данных (callType = token0 ключа ≠ 0 / 1 → revert()), весь обмен откатился без данных, владелец при своих`);
  }
  return record;
}
