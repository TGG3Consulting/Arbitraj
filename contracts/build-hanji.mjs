// Builds the standalone Hanji execution contract with the compiler in contracts/package-lock.json.
// The original ArbExecutor and its versioned artifacts use their existing independent build.
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import solc from 'solc';

const output = process.argv[2];
if (!output) throw new Error('Expected an artifact directory');
const file = 'HanjiExecutor.sol';
const root = import.meta.dirname;
const input = {
  language: 'Solidity',
  sources: { [file]: { content: readFileSync(join(root, file), 'utf8').replace(/^\uFEFF/, '') } },
  settings: {
    optimizer: { enabled: true, runs: 200 }, viaIR: true, evmVersion: 'cancun',
    outputSelection: { '*': { '*': ['abi', 'evm.bytecode.object', 'evm.deployedBytecode.object', 'storageLayout'] } },
  },
};
const result = JSON.parse(solc.compile(JSON.stringify(input)));
for (const error of (result.errors ?? [])) {
  if (error.severity === 'error') throw new Error(error.formattedMessage);
}
const contract = result.contracts[file].HanjiExecutor;
if (!contract.evm.deployedBytecode.object || contract.evm.deployedBytecode.object.length / 2 > 24576)
  throw new Error('HanjiExecutor exceeds the EVM contract size limit or is empty');
mkdirSync(output, { recursive: true });
writeFileSync(join(output, 'HanjiExecutor.json'), JSON.stringify({
  solc: solc.version(), abi: contract.abi, creation: contract.evm.bytecode.object,
  runtime: contract.evm.deployedBytecode.object, storage: contract.storageLayout.storage,
}));
