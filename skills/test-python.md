# Skill: Run Python Tests on OpenVPS

Run your Python test suite on a fresh cloud server. Supports pytest, unittest, and tox.

## Example: Run pytest

```json
POST https://openvps.sh/v1/jobs
{
  "command": "cd /root/project && python -m pytest -v",
  "setup": "apt-get update && apt-get install -y python3 python3-pip python3-venv && cd /root/project && pip3 install -r requirements.txt",
  "files": {
    "/root/project/requirements.txt": "pytest>=8.0\nrequests",
    "/root/project/test_example.py": "def test_add():\n    assert 1 + 2 == 3\n\ndef test_string():\n    assert 'hello'.upper() == 'HELLO'"
  },
  "vcpus": 2,
  "ram_mb": 1024,
  "timeout": 300
}
```

## Example: Django tests

```json
{
  "command": "cd /root/project && python manage.py test",
  "setup": "apt-get update && apt-get install -y python3 python3-pip && cd /root/project && pip3 install -r requirements.txt",
  "files": { ... },
  "vcpus": 2,
  "ram_mb": 2048,
  "timeout": 600
}
```

## Frameworks

| Framework | Setup | Command |
|-----------|-------|---------|
| pytest | `pip3 install pytest` | `python -m pytest -v` |
| unittest | built-in | `python -m unittest discover` |
| tox | `pip3 install tox` | `tox` |
| Django | `pip3 install django` | `python manage.py test` |
| Flask | `pip3 install flask pytest` | `python -m pytest` |
