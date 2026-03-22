# Skill: Run JavaScript/TypeScript Tests on OpenVPS

Run your JavaScript or TypeScript test suite on a fresh cloud server. Supports Node.js, Bun, Vitest, Jest, and Mocha.

## Example: Run Vitest

```json
POST https://openvps.sh/v1/jobs
{
  "command": "cd /root/project && npm test",
  "setup": "curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && apt-get install -y nodejs && cd /root/project && npm install",
  "files": {
    "/root/project/package.json": "{\"scripts\":{\"test\":\"vitest run\"},\"devDependencies\":{\"vitest\":\"^3.0.0\"}}",
    "/root/project/sum.test.js": "import { expect, test } from 'vitest';\ntest('adds', () => expect(1 + 2).toBe(3));"
  },
  "vcpus": 2,
  "ram_mb": 2048,
  "timeout": 300
}
```

## Example: Run with Bun

```json
{
  "command": "cd /root/project && bun test",
  "setup": "curl -fsSL https://bun.sh/install | bash && export PATH=$HOME/.bun/bin:$PATH && cd /root/project && bun install",
  "files": {
    "/root/project/package.json": "...",
    "/root/project/src/index.test.ts": "..."
  },
  "vcpus": 2,
  "ram_mb": 1024,
  "timeout": 300
}
```

## Example: Upload full project via files

For small projects, include all source files in the `files` map. For larger projects, use the `ssh_host`/`ssh_port` from the response to rsync before the job runs:

```bash
# 1. Submit job (gets VM + SSH access)
RESP=$(curl -s ... POST /v1/jobs ...)
SSH_HOST=$(echo $RESP | jq -r .ssh_host)
SSH_PORT=$(echo $RESP | jq -r .ssh_port)

# 2. rsync your project (uses the provision's SSH key)
rsync -avz -e "ssh -p $SSH_PORT -i vm_key" ./my-project/ root@$SSH_HOST:/root/project/
```

## Frameworks

| Framework | Setup | Command |
|-----------|-------|---------|
| Vitest | `npm install` | `npx vitest run` |
| Jest | `npm install` | `npx jest` |
| Mocha | `npm install` | `npx mocha` |
| Bun test | `bun install` | `bun test` |
| Node test runner | none | `node --test` |
