"""Admission/participation controls only; never starts a product or substitutes a corpus."""
import asyncio
import importlib.util
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


bench = load('real_document_operations_tested', HERE / 'real-document-operations.py')


class AdmissionTests(unittest.TestCase):
    def test_legacy_global64_still_limits_total_whole_workflows(self):
        spec = {'writes': 1}
        self.assertEqual(bench.plan.admission(spec, 1000, 1000, 4, 'global', 64, 0, None), (64, 64))
        self.assertEqual(bench.plan.admission(spec, 1000, 1000, 4, 'global', 64, 64, None), (0, 0))

    def test_family_budgets_schedule_all_writers_independently(self):
        for writes in (1, 2, 8, 10, 13, 24):
            self.assertEqual(bench.plan.admission({'writes': writes}, 1000, 1000, 4,
                'family', 64, 100000, None), (1000, 1000 * writes))

    def test_explicit_family_cap_reserves_complete_workflows_and_real_targets(self):
        spec = {'writes': 13}
        self.assertEqual(bench.plan.admission(spec, 1000, 1000, 4, 'family', 64, 0, 29), (2, 29))
        self.assertEqual(bench.plan.admission(spec, 1000, 1, 4, 'family', 64, 0, None), (1, 13000))
        self.assertEqual(bench.plan.admission(spec, 1000, 1000, 4, 'family', 64, 0, 0), (0, 0))
        self.assertEqual(bench.plan.admission({'render': True}, 1000, 60, 4, 'family', 64, 0, None), (4, None))

    def test_manifest_retains_unmeasured_and_partial_historical_scopes(self):
        value = bench.plan.manifest(HERE / 'real-document-operations.json')
        self.assertEqual(len(value['families']), 32)
        self.assertEqual(sum(x.get('execution') == 'implemented' for x in value['families']), 23)
        self.assertEqual(len([x for x in value['families'] if x.get('execution') != 'implemented']), 9)
        self.assertEqual({x['family'] for x in value['historicalFamilyRemainders']},
            {'variables-themes', 'reparent-reorder-group-align'})
        self.assertTrue(all(x['remainingUnmeasured'] for x in value['historicalFamilyRemainders']))

    def test_cli_rejects_unknown_family_and_mixed_budget_modes_before_open(self):
        base = [sys.executable, str(HERE / 'real-document-operations.py'),
            '--build-manifest', '/does-not-exist', '--engine-commit', 'none',
            '--server-sha', 'none', '--fixture', '/does-not-exist', '--output', '/must-not-be-created']
        for arguments, expected in [(['--family', 'made-up-op'], 'invalid choice'),
                (['--write-budget-scope', 'family', '--write-budget', '64'], 'use --family-write-budget'),
                (['--family-write-budget', '64'], 'requires --write-budget-scope family')]:
            result = subprocess.run(base + arguments, capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn(expected, result.stderr)


class PhaseTests(unittest.IsolatedAsyncioTestCase):
    def case(self, clients=10, scope='family'):
        case = object.__new__(bench.Case)
        case.args = SimpleNamespace(render_budget=4, max_raster_edge=1024,
            write_budget_scope=scope, write_budget=64, family_write_budget=None)
        case.clients, case.inflight, case.writes = clients, 32, 0
        case.fixture = {'id': 'UNIT-CONTROL-NOT-A-BENCHMARK',
                        'worksets': {'geometry': [{'id': str(i)} for i in range(clients)]}}
        case.catalog, case.report = {'write'}, {'coverage': {}, 'phases': {}}
        case.save = lambda: None
        case.event = lambda *args, **kwargs: None
        case.current = None
        return case

    async def run_phase(self, case, workflow, metadata=False):
        case.workflow = workflow
        spec = {'family': 'unit-control', 'tools': ['write'], 'execution': 'implemented',
                'writes': 1, 'role': 'document-metadata' if metadata else 'geometry', 'serialized': metadata}
        with patch.object(bench.shared, 'phase'), patch.object(bench.shared, 'usage', return_value=None):
            await case.phase(spec)
        return case.report['phases']['unit-control']

    async def test_family_counts_actual1000_writer_participants_not_shared64(self):
        case = self.case(1000)
        case.process = SimpleNamespace(pid=0)

        async def workflow(family, target, agent):
            case.report['phases'][family]['requests'].append({'agent': agent, 'tool': 'write',
                'write': True, 'accepted': True, 'latencySeconds': 0})

        row = await self.run_phase(case, workflow)
        self.assertEqual((row['scheduledClients'], row['startedClients'], row['acceptedWorkflows']), (1000, 1000, 1000))
        self.assertEqual(row['acceptedMutationParticipants'], 1000)
        self.assertEqual(case.report['coverage']['unit-control']['status'], 'measured')

    async def test_metadata_writers_share_actual_document_scope_not_fake_node_targets(self):
        case = self.case(10)
        case.process = SimpleNamespace(pid=0)

        async def workflow(family, target, agent):
            self.assertIs(target, case.fixture)

        row = await self.run_phase(case, workflow, metadata=True)
        self.assertEqual((row['scheduledClients'], row['startedClients'], row['maxInflight']), (10, 10, 1))
        self.assertEqual(row['distinctTargets'], 1)
        self.assertEqual(row['acceptedMutationParticipants'], 0)  # No RPC fabricated by this unit control.

    async def test_refusals_are_visible_and_not_coverage_passes(self):
        case = self.case(2)
        case.process = SimpleNamespace(pid=0)

        async def workflow(family, target, agent):
            if agent == 0:
                raise bench.Rejected('admission')

        row = await self.run_phase(case, workflow)
        self.assertEqual((row['acceptedWorkflows'], row['rejectedWorkflows']), (1, 1))
        self.assertEqual(case.report['coverage']['unit-control']['status'], 'measured-with-refusals')

    async def test_failure_cancels_queued_work_without_claiming_scheduled_writers(self):
        case = self.case(1000)
        case.process = SimpleNamespace(pid=0)

        async def workflow(family, target, agent):
            await asyncio.sleep(0)
            if agent == 0:
                raise AssertionError('unit fault')
            await asyncio.Event().wait()

        with self.assertRaisesRegex(AssertionError, 'unit fault'):
            await self.run_phase(case, workflow)
        row = case.report['phases']['unit-control']
        self.assertEqual(row['scheduledClients'], 1000)
        self.assertLess(row['startedClients'], 1000)
        self.assertLessEqual(row['peakInflight'], 32)
        self.assertGreater(row['cancelledWorkflows'], 0)
        self.assertEqual(row['acceptedMutationParticipants'], 0)
        self.assertEqual(case.report['coverage']['unit-control']['status'], 'failed-correctness')


if __name__ == '__main__':
    unittest.main()
