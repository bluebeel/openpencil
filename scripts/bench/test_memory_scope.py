"""Ensure benchmark scope changes cannot silently disable resource enforcement."""
import asyncio
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("shared_benchmark", Path(__file__).with_name("headless-agents.py"))
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


class Process:
    pid = 42
    returncode = None

    def terminate(self):
        self.returncode = -15


class MemoryScopeTests(unittest.TestCase):
    def measure(self, scope, service_rss=200, service_physical=200):
        process, report = Process(), {}

        def usage(pid):
            return {"rss": service_rss if pid == 42 else 200,
                    "footprint": service_physical if pid == 42 else 200}

        async def finish(_):
            process.returncode = 0

        with patch.object(shared, "usage", side_effect=usage), patch.object(shared, "USAGE_API", True), \
             patch.object(shared.asyncio, "sleep", side_effect=finish), \
             patch.object(shared.resource, "getrusage", return_value=SimpleNamespace(ru_maxrss=0)):
            asyncio.run(shared.monitor(process, report, 300, scope))
        return process, report

    def test_default_aggregate_scope_still_enforces_generator_bytes(self):
        process, report = self.measure("aggregate")
        self.assertEqual(process.returncode, -15)
        self.assertTrue(report["memoryBudgetExceeded"])

    def test_service_scope_records_aggregate_and_charges_requested_process(self):
        process, report = self.measure("service")
        self.assertEqual(process.returncode, 0)
        self.assertEqual(report["peakAggregateRssBytes"], 400)
        self.assertEqual(report["peakServiceRssBytes"], 200)
        self.assertNotIn("memoryBudgetExceeded", report)

    def test_service_scope_enforces_rss_and_physical_separately(self):
        for rss, physical in [(400, 200), (200, 400)]:
            process, report = self.measure("service", rss, physical)
            self.assertEqual(process.returncode, -15)
            self.assertTrue(report["memoryBudgetExceeded"])


if __name__ == "__main__":
    unittest.main()
