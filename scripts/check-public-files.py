#!/usr/bin/env python3
"""Bounded public-source hygiene check; never prints matched credential values."""
from pathlib import Path
import re
import os
import sys

root = Path(__file__).resolve().parent.parent
excluded = {'.git', 'target', 'artifacts', '__pycache__'}
patterns = {
    'private absolute path': re.compile(r'/(?:Users|Volumes|private/var/folders)/[^\s]+'),
    'private key': re.compile(r'-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----'),
    'GitHub token': re.compile(r'(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})'),
    'AWS access key': re.compile(r'AKIA[0-9A-Z]{16}'),
    'OpenAI token': re.compile(r'sk-(?:proj-|svcacct-)[A-Za-z0-9_-]{30,}'),
}
failures = []
checked = 0
def public_files():
    for directory, folders, files in os.walk(root):
        folders[:] = sorted(folder for folder in folders if folder not in excluded)
        for name in sorted(files):
            yield Path(directory) / name


for path in public_files():
    relative = path.relative_to(root)
    if path.is_symlink():
        failures.append(f'{relative}: symlink requires explicit public review')
        continue
    if not path.is_file():
        continue
    if path.name == '.DS_Store' or path.suffix in {'.pem', '.p12', '.key'} or path.name.startswith('.env'):
        failures.append(f'{relative}: private/cache file')
    try:
        source = path.read_text(encoding='utf-8')
    except UnicodeDecodeError:
        if path.suffix != '.png':
            failures.append(f'{relative}: unexpected binary file')
        continue
    checked += 1
    for label, pattern in patterns.items():
        if pattern.search(source):
            failures.append(f'{relative}: {label}')
if failures:
    print('\n'.join(failures))
    sys.exit(1)
print(f'PASS: {checked} public text files checked; bounded credential/path/cache audit.')
