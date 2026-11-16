#!/usr/bin/env python3
"""Run regression tests without modifying desktop preferences or stopping containers."""
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent

def run(*args, env=None):
    subprocess.run(args, cwd=root, env=env, check=True)

for suite in ('sockets', 'extension'):
    run('node', '--experimental-default-type=module', f'tests/{suite}.test.mjs')
run('gjs', '-m', 'tests/runtime.test.js')
with tempfile.TemporaryDirectory(prefix='port-killer-tests-') as tmp:
    schemas = Path(tmp) / 'schemas'
    schemas.mkdir()
    for source in (root / 'extension' / 'schemas').glob('*.gschema.xml'):
        (schemas / source.name).write_bytes(source.read_bytes())
    run('glib-compile-schemas', '--strict', str(schemas))
    env = dict(os.environ, GSETTINGS_SCHEMA_DIR=str(schemas),
               GSETTINGS_BACKEND='keyfile', XDG_CONFIG_HOME=tmp)
    run('gjs', '-m', 'tests/preferences.test.js', 'write', env=env)
    run('gjs', '-m', 'tests/preferences.test.js', 'read', env=env)
# Install into a temporary XDG data directory, never the active desktop.
with tempfile.TemporaryDirectory(prefix='port-killer-install-') as tmp:
    env = dict(os.environ, XDG_DATA_HOME=tmp)
    run('bash', 'install.sh', env=env)
    installed = Path(tmp) / 'gnome-shell' / 'extensions' / 'port-killer@dev'
    for name in ('extension.js', 'runtime.js', 'sockets.js', 'metadata.json', 'stylesheet.css',
                 'schemas/gschemas.compiled'):
        assert (installed / name).is_file(), f'Missing installed file: {name}'
    print('PASS: extension installer packages the new folder structure and compiled schema')
print('All regression checks passed.')
