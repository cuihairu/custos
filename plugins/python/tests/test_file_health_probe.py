"""Path-safety tests for the file_health_probe example (todo.md S1).

Run with: python3 -m unittest discover -s plugins/python/tests
Stdlib only: the examples must stay dependency-free, so the harness is
unittest rather than pytest.
"""

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "examples"))

import file_health_probe


class FileHealthProbePathSafety(unittest.TestCase):
    def setUp(self):
        self.base = tempfile.mkdtemp(prefix="custos-s1-base-")
        self.old_target = file_health_probe.TARGET
        file_health_probe.TARGET = os.path.join(self.base, "default.target")

    def tearDown(self):
        file_health_probe.TARGET = self.old_target

    def test_probe_refuses_absolute_targets_outside_the_base(self):
        with tempfile.TemporaryDirectory() as outside:
            victim = os.path.join(outside, "victim.txt")
            with open(victim, "w"):
                pass
            result = file_health_probe.probe({"args": {"target": victim}})
        self.assertFalse(result["healthy"], result)
        self.assertIn("refused", result["observation"])

    def test_recover_refuses_absolute_targets_outside_the_base(self):
        with tempfile.TemporaryDirectory() as outside:
            victim = os.path.join(outside, "victim.txt")
            result = file_health_probe.recover({"args": {"target": victim}})
            self.assertFalse(
                os.path.exists(victim), "recovery touched a file outside the base"
            )
        self.assertFalse(result["succeeded"], result)
        self.assertIn("refused", result["detail"])

    def test_recover_refuses_parent_directory_traversal(self):
        # Run from inside a controlled base so the OLD behavior (raw path
        # use) would create ../escaped.txt inside this temp tree.
        with tempfile.TemporaryDirectory() as parent:
            base = os.path.join(parent, "base")
            os.makedirs(base)
            old_target = file_health_probe.TARGET
            file_health_probe.TARGET = os.path.join(base, "default.target")
            os.chdir(base)
            try:
                result = file_health_probe.recover(
                    {"args": {"target": "../escaped.txt"}}
                )
            finally:
                os.chdir(os.path.dirname(os.path.abspath(__file__)))
                file_health_probe.TARGET = old_target
            self.assertFalse(result["succeeded"], result)
            self.assertIn("refused", result["detail"])
            self.assertFalse(
                os.path.exists(os.path.join(parent, "escaped.txt")),
                "traversal escaped the base directory",
            )

    def test_relative_targets_inside_the_base_still_work(self):
        inner = os.path.join(self.base, "inner.txt")
        with open(inner, "w"):
            pass
        result = file_health_probe.probe({"args": {"target": "inner.txt"}})
        self.assertTrue(result["healthy"], result)
        touched = file_health_probe.recover({"args": {"target": "inner.txt"}})
        self.assertTrue(touched["succeeded"], touched)

    def test_default_target_still_used_without_params(self):
        result = file_health_probe.recover({})
        self.assertTrue(result["succeeded"], result)
        self.assertTrue(os.path.exists(file_health_probe.TARGET))


if __name__ == "__main__":
    unittest.main()
