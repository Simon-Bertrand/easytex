import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, join } from 'node:path';
import { spawnSync } from 'node:child_process';

const root = join(import.meta.dirname, '..', '..');

function cargoCandidates() {
  const candidates = [];
  if (process.env.CARGO) candidates.push(process.env.CARGO);
  candidates.push('cargo');

  if (process.platform === 'win32') {
    candidates.push(join(process.env.USERPROFILE || homedir(), '.cargo', 'bin', 'cargo.exe'));
  } else {
    candidates.push(join(process.env.HOME || homedir(), '.cargo', 'bin', 'cargo'));
  }

  return candidates;
}

function commandExists(command) {
  if (command.includes('/') || command.includes('\\')) {
    return existsSync(command);
  }

  const extensions = process.platform === 'win32'
    ? (process.env.PATHEXT || '.EXE;.CMD;.BAT').split(';')
    : [''];

  for (const dir of (process.env.PATH || '').split(delimiter)) {
    for (const ext of extensions) {
      if (existsSync(join(dir, command + ext))) return true;
    }
  }
  return false;
}

const cargo = cargoCandidates().find(commandExists);
if (!cargo) {
  console.error('Unable to find cargo. Set CARGO=/path/to/cargo and retry.');
  process.exit(127);
}

const result = spawnSync(cargo, ['run', '-p', 'xtask', '--', 'generate-types'], {
  cwd: root,
  stdio: 'inherit',
  env: process.env,
});

process.exit(result.status ?? 1);
