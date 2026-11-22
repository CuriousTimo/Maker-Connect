# The start of every Python driver of the PrusaLink tests (run with `reference/.venv-prusalink/bin/python
# -I -W ignore` or $PRUSALINK_PYTHON, argv[1] = PrusaLink's checkout, argv[2] = the SDK's): PrusaLink's own modules import packages of its
# device (prctl, pyudev, websocket, ...) that the test venv does not have and that none of the code the
# tests run uses; they are stubbed, nothing else is.
import importlib.abc
import importlib.machinery
import json
import sys
from unittest.mock import MagicMock

LINK, SDK = sys.argv[1], sys.argv[2]
STUB = {'prctl', 'pyudev', 'websocket', 'pyric', 'sdnotify', 'psutil'}


class _Stub(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    def find_spec(self, name, path, target=None):
        if name.split('.')[0] in STUB:
            return importlib.machinery.ModuleSpec(name, self, is_package=True)

    def create_module(self, spec):
        module = MagicMock()
        module.__path__ = []
        module.__spec__ = spec
        module.__name__ = spec.name
        return module

    def exec_module(self, module):
        pass


sys.meta_path.append(_Stub())
sys.path[:0] = [LINK, SDK]
