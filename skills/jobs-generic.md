# Skill: Run Jobs on OpenVPS

Run any command on a fresh Ubuntu 24.04 server. Pay with USDC, get results back. VM auto-terminates when done.

## Quick Start

```bash
# 1. Submit a job
curl -s https://openvps.sh/v1/jobs \
  -H "Content-Type: application/json" \
  -d '{"command": "echo hello world", "timeout": 120}'
# → 402 Payment Required (pay USDC on Base, Celo, or Tempo)

# 2. After payment → job starts
# → {"job_id": "...", "status": "running", "ssh_host": "...", "ssh_port": 2201}

# 3. Poll for results
curl -s https://openvps.sh/v1/jobs/JOB_ID
# → {"status": "completed", "exit_code": 0, "output": "hello world", "duration_secs": 2}
```

## Request Format

```json
{
  "command": "your command here",
  "setup": "apt-get update && apt-get install -y python3",
  "files": {
    "/root/script.py": "print('hello from python')",
    "/root/data.json": "{\"key\": \"value\"}"
  },
  "vcpus": 2,
  "ram_mb": 2048,
  "timeout": 600
}
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| command | yes | — | Shell command to run |
| setup | no | — | Setup script (runs first, install deps) |
| files | no | — | Map of file path → content to upload before running |
| vcpus | no | 1 | CPUs (1-4) |
| ram_mb | no | 512 | RAM in MB (256-4096) |
| timeout | no | 300 | Max seconds (60-3600) |

## Execution Order

1. VM boots (~3s)
2. Files uploaded (if any)
3. Setup script runs (if any)
4. Main command runs
5. Output captured
6. VM terminated and deleted

## Tips

- The VM has internet access — you can `apt install`, `pip install`, `curl`, etc.
- Max timeout is 60 minutes (3600 seconds)
- Output includes both stdout and stderr
- The `ssh_host` and `ssh_port` in the response let you also SSH in manually or rsync files
- Each job gets a completely fresh, isolated VM
