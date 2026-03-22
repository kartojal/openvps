# Skill: Run Solidity Tests on OpenVPS

Run your Solidity smart contract tests on a fresh cloud server. Supports Foundry (forge) and Hardhat.

## Example: Foundry (forge test)

```json
POST https://openvps.sh/v1/jobs
{
  "command": "source $HOME/.bashrc && cd /root/project && forge test -vvv 2>&1",
  "setup": "curl -L https://foundry.paradigm.xyz | bash && source $HOME/.bashrc && foundryup",
  "files": {
    "/root/project/foundry.toml": "[profile.default]\nsrc = \"src\"\nout = \"out\"\nlibs = [\"lib\"]\n",
    "/root/project/src/Counter.sol": "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.20;\n\ncontract Counter {\n    uint256 public number;\n    function increment() public { number++; }\n}",
    "/root/project/test/Counter.t.sol": "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.20;\nimport \"forge-std/Test.sol\";\nimport \"../src/Counter.sol\";\n\ncontract CounterTest is Test {\n    Counter counter;\n    function setUp() public { counter = new Counter(); }\n    function test_Increment() public { counter.increment(); assertEq(counter.number(), 1); }\n}"
  },
  "vcpus": 2,
  "ram_mb": 2048,
  "timeout": 300
}
```

## Example: Hardhat

```json
{
  "command": "cd /root/project && npx hardhat test 2>&1",
  "setup": "curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && apt-get install -y nodejs && cd /root/project && npm install",
  "files": {
    "/root/project/package.json": "{\"devDependencies\":{\"hardhat\":\"^2.22.0\",\"@nomicfoundation/hardhat-toolbox\":\"^5.0.0\"}}",
    "/root/project/hardhat.config.js": "require('@nomicfoundation/hardhat-toolbox');\nmodule.exports = { solidity: '0.8.20' };",
    "/root/project/contracts/Counter.sol": "...",
    "/root/project/test/Counter.js": "..."
  },
  "vcpus": 2,
  "ram_mb": 2048,
  "timeout": 600
}
```

## Frameworks

| Framework | Setup | Command |
|-----------|-------|---------|
| Foundry | `foundryup` | `forge test -vvv` |
| Hardhat | `npm install` | `npx hardhat test` |
| Truffle | `npm install` | `npx truffle test` |

## Tips

- Foundry installs fast (~15s) and runs tests in-process (no Node needed)
- Use `forge install` in setup to pull dependencies from GitHub
- For fork tests, pass `--fork-url` with an RPC URL
