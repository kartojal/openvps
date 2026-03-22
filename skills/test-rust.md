# Skill: Run Rust Tests on OpenVPS

Run your Rust test suite on a fresh cloud server with cargo.

## Example: Run cargo test

```json
POST https://openvps.sh/v1/jobs
{
  "command": "cd /root/project && cargo test 2>&1",
  "setup": "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && source $HOME/.cargo/env && apt-get update && apt-get install -y build-essential pkg-config libssl-dev",
  "files": {
    "/root/project/Cargo.toml": "[package]\nname = \"example\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    "/root/project/src/lib.rs": "#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_works() {\n        assert_eq!(2 + 2, 4);\n    }\n}"
  },
  "vcpus": 4,
  "ram_mb": 4096,
  "timeout": 600
}
```

## Tips

- Use `vcpus: 4` and `ram_mb: 4096` for Rust — compilation is CPU/RAM heavy
- First build takes longer (downloading + compiling deps). Set `timeout: 600` or more
- The setup installs Rust via rustup — takes ~30s
- For large projects, rsync the source via SSH instead of using `files`

## For larger projects (rsync approach)

```bash
# 1. Create a job with just the build command (no files)
RESP=$(curl -s POST https://openvps.sh/v1/jobs ...)

# 2. rsync your project to the VM
SSH_HOST=$(echo $RESP | jq -r .ssh_host)
SSH_PORT=$(echo $RESP | jq -r .ssh_port)
rsync -avz -e "ssh -p $SSH_PORT -i vm_key" \
  --exclude target \
  ./my-rust-project/ root@$SSH_HOST:/root/project/
```
