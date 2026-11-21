// Файл: contracts/build.mjs
// Что делает: сборка ArbExecutor.sol компилятором solc-js фиксированной версии (package.json, package-lock.json):
//   стандартный JSON-вход solc, оптимизатор, конвейер via-IR (решение 457 — только ArbExecutor), EVM cancun
//   (transient-переменные EIP-1153; BSC — с хардфорка Haber).
//   Пишет артефакты в contracts/out/: ArbExecutor.bin (код создания), ArbExecutor.bin-runtime (код в сети — его
//   подставляет «Сухая сделка DEX»), ArbExecutor.abi (ABI JSON — из него alloy sol! в Rust), ArbExecutor.layout.json
//   (раскладка хранилища — слот владельца), ArbExecutor.sha256 (хеши артефактов и версия компилятора).
// Кто вызывает: contracts/build.sh.
// От чего зависит: npm solc (версия — package.json), contracts/ArbExecutor.sol, contracts/IPools.sol,
//   contracts/VirtualQuoter.sol (решение 186 — отдельная сборка, out/VirtualQuoter.*).
// Настройки: нет (параметры сборки — здесь и в README.md; меняются только вместе с пересборкой и коммитом out/).
//
// Формат входа/выхода — https://docs.soliditylang.org/en/v0.8.37/using-the-compiler.html#compiler-input-and-output-json-description

import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import solc from 'solc';

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, 'out');
const sources = ['ArbExecutor.sol', 'IPools.sol'];

const input = {
  language: 'Solidity',
  sources: Object.fromEntries(sources.map((f) => [f, { content: readFileSync(join(here, f), 'utf8') }])),
  settings: {
    optimizer: { enabled: true, runs: 1 },
    // Решение 488 (v5): v5 со всеми шагами 26–31 при runs 200 = 26 734 > EIP-170 24 576; runs 1 + strip
    // revert = 24 542 (запас 34 байта). Газ считается из симуляции бандла (bundle_sim), учтён автоматически.
    // Следующий новый DEX потребует разбивки исполнителя на библиотеку (T488-01). Только ArbExecutor.
    debug: { revertStrings: 'strip' },
    // Решение 457: исполнитель v4 — конвейер via-IR (settings.viaIR, «Input Description» —
    // https://docs.soliditylang.org/en/v0.8.37/using-the-compiler.html). Причина — предел размера кода EIP-170
    // (24 576 байт): обычным конвейером runtime v4 — 26 330 байт (runs 1 — 26 030), via-IR + runs 200 — 21 775.
    // Код контракта не менялся. Только для ArbExecutor; VirtualQuoter ниже — прежний конвейер (артефакты прежние).
    // Артефакты v3 (out/v3/) собраны без via-IR и заморожены.
    viaIR: true,
    evmVersion: 'cancun',
    outputSelection: {
      'ArbExecutor.sol': {
        ArbExecutor: ['abi', 'evm.bytecode.object', 'evm.deployedBytecode.object', 'storageLayout'],
      },
    },
  },
};

const result = JSON.parse(solc.compile(JSON.stringify(input)));
let failed = false;
for (const e of result.errors ?? []) {
  console.error(e.formattedMessage);
  if (e.severity === 'error') failed = true;
}
if (failed) process.exit(1);

const c = result.contracts['ArbExecutor.sol'].ArbExecutor;
mkdirSync(out, { recursive: true });
const files = {
  'ArbExecutor.bin': c.evm.bytecode.object + '\n',
  'ArbExecutor.bin-runtime': c.evm.deployedBytecode.object + '\n',
  'ArbExecutor.abi': JSON.stringify(c.abi, null, 2) + '\n',
  'ArbExecutor.layout.json': JSON.stringify(c.storageLayout, null, 2) + '\n',
};
const lines = [`solc ${solc.version()}`];
for (const [name, text] of Object.entries(files)) {
  writeFileSync(join(out, name), text);
  lines.push(`${createHash('sha256').update(text).digest('hex')}  ${name}`);
}
writeFileSync(join(out, 'ArbExecutor.sha256'), lines.join('\n') + '\n');
console.log(lines.join('\n'));

// Решение 186: виртуальный квотер — ОТДЕЛЬНОЙ сборкой (своё задание solc), чтобы артефакты ArbExecutor не менялись.
// В сети не разворачивается: бот подставляет его код в eth_call (src/venues/bsc_dex/quote/virtual_quoter.rs).
const vqInput = {
  language: 'Solidity',
  sources: { 'VirtualQuoter.sol': { content: readFileSync(join(here, 'VirtualQuoter.sol'), 'utf8') } },
  settings: {
    optimizer: { enabled: true, runs: 200 },
    evmVersion: 'cancun',
    outputSelection: { 'VirtualQuoter.sol': { VirtualQuoter: ['abi', 'evm.deployedBytecode.object'] } },
  },
};
const vq = JSON.parse(solc.compile(JSON.stringify(vqInput)));
for (const e of vq.errors ?? []) {
  console.error(e.formattedMessage);
  if (e.severity === 'error') process.exit(1);
}
const v = vq.contracts['VirtualQuoter.sol'].VirtualQuoter;
const vqFiles = {
  'VirtualQuoter.bin-runtime': v.evm.deployedBytecode.object + '\n',
  'VirtualQuoter.abi': JSON.stringify(v.abi, null, 2) + '\n',
};
const vqLines = [`solc ${solc.version()}`];
for (const [name, text] of Object.entries(vqFiles)) {
  writeFileSync(join(out, name), text);
  vqLines.push(`${createHash('sha256').update(text).digest('hex')}  ${name}`);
}
writeFileSync(join(out, 'VirtualQuoter.sha256'), vqLines.join('\n') + '\n');
console.log(vqLines.join('\n'));
