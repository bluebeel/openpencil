#!/usr/bin/env python3
"""Bounded real-document MCP operation matrix, never a replacement toy board.

Inputs are a full immutable .op plus a compact target
inventory. Each case starts a fresh copy and one actual shared server. Missing
tools, insufficient distinct targets, write limits and deadlines stay explicit.
"""
import argparse
import asyncio
import base64
import collections
import gc
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import resource
import shutil
import struct
import sys
import time

HERE = Path(__file__).resolve()
if (HERE.parent / 'headless-agents.py').exists():
    ENGINE = HERE.parents[2]
    ROOT = ENGINE.parents[1]
else:
    ROOT = next(parent for parent in HERE.parents if (parent / 'engines/openpencil/scripts/bench/headless-agents.py').exists())
    ENGINE = ROOT / 'engines/openpencil'


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


shared = module('real_document_shared', ENGINE / 'scripts/bench/headless-agents.py')
pixels = module('real_document_pixels', ENGINE / 'scripts/bench/operation-matrix.py')
families = module('real_document_families', HERE.with_name('real-document-families.py'))
plan = module('real_document_plan', HERE.with_name('real-document-plan.py'))


def checkpoint(path, value):
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def semantic_hash(value):
    digest = hashlib.sha256()

    def walk(item):
        if item is None:
            digest.update(b'n')
        elif isinstance(item, bool):
            digest.update(b't' if item else b'f')
        elif isinstance(item, int):
            digest.update(b'i' + str(item).encode() + b';')
        elif isinstance(item, float):
            if item.is_integer() and not (item == 0 and math.copysign(1, item) < 0):
                digest.update(b'i' + str(int(item)).encode() + b';')
            else:
                digest.update(b'd' + struct.pack('>d', item))
        elif isinstance(item, str):
            data = item.encode()
            digest.update(b's' + str(len(data)).encode() + b':' + data)
        elif isinstance(item, list):
            digest.update(b'[')
            for child in item:
                walk(child)
            digest.update(b']')
        elif isinstance(item, dict):
            digest.update(b'{')
            for key in sorted(item):
                walk(key)
                walk(item[key])
            digest.update(b'}')
        else:
            raise TypeError(type(item))
    walk(value)
    return digest.hexdigest()


def expect_saved_active_page(document, index):
    """Model only the host's explicit editorMeta save extension, not UI drops."""
    assert isinstance(index, int) and not isinstance(index, bool) and index >= 0
    if 'editorMeta' not in document:
        # A fresh workcopy has no legacy sidecar. The source loader's absent
        # metadata default is false; Save always emits this typed extension.
        document['editorMeta'] = {'preserveAuthoredGeometry': False}
    assert isinstance(document['editorMeta'], dict)
    document['editorMeta']['activePageIndex'] = index


class Rejected(Exception):
    pass


class Case(families.FamilyWorkflows):
    def __init__(self, args, fixture, clients, inflight, out, manifest, pin):
        self.args, self.fixture, self.clients = args, fixture, clients
        self.inflight, self.out, self.manifest, self.pin = inflight, out, manifest, pin
        self.process = self.sampler = self.observer = None
        self.port, self.serial, self.writes = None, 0, 0
        self.current, self.expected, self.uncertain = None, {}, {}
        self.accepted_active_pages, self.saved_active_page = {}, None
        self.report = {'status': 'starting', 'logicalClients': clients,
            'requestedActiveConcurrency': inflight, 'oneSharedService': True,
            'fullDocument': True, 'fixture': fixture, 'binaryPin': pin,
            'externalLlmProcesses': 0, 'phases': {}, 'coverage': {},
            'requestRetryPolicy': 'none; no ambiguous write is replayed',
            'writeBudgetRequests': args.write_budget if args.write_budget_scope == 'global' else None,
            'renderBudgetRequests': args.render_budget,
            'persistenceMode': args.persistence,
            'writeBudgetScope': args.write_budget_scope,
            'perFamilyWriteBudget': args.family_write_budget,
            'familyBudgetDefault': 'Explicit family mode reserves clients × workflow writes independently; default global mode preserves the earlier64-request budget',
            'historicalFamilyRemainders': manifest.get('historicalFamilyRemainders', []),
            'persistenceContract': ('Bound Managed primary command edits save atomically before success; the final explicit save is a verification checkpoint. No fsync/power-loss guarantee or automatic durability for every REST/direct filesystem mutator is claimed.' if args.managed_primary_save_before_ack else 'Live mutation RPC and explicit save RPC are separate; durable workflow mode waits for save after each confirmed mutation. It is not atomic mutation+save in one RPC.'),
            'managedPrimarySaveBeforeAckConfigured': args.managed_primary_save_before_ack,
            'perAcknowledgementCrashRecoveryTested': False,
            'memoryTargetBytes': 1_000_000_000, 'memoryAllowanceBytes': args.max_bytes,
            'diagnosticAllowance': args.max_bytes > 1_000_000_000,
            'latencyScope': 'Workflow latency includes its setup, verification and cleanup calls; per-tool samples are also reported.',
            'fontParityScope': 'Resource inventory only. A produced PNG does not prove requested-font identity or Figma pixel parity.',
            'arbitraryCorpusImagePixelIdentityVerified': False,
            'rasterCorrectnessScope': 'PNG dimensions/structure/hash and source image closure are recorded. Arbitrary corpus image pixels are not claimed to match an oracle; synthetic SVG center control is checked separately.'}

    def save(self):
        if hasattr(self, 'catalog'):
            invocations = collections.Counter(row['tool'] for phase in self.report['phases'].values() for row in phase.get('requests', []))
            self.report['liveToolInvocationCounts'] = {name: invocations[name] for name in sorted(self.catalog)}
            self.report['liveToolsNotInvoked'] = sorted(self.catalog - invocations.keys())
            self.report['mutationParticipation'] = plan.participation([
                row for phase in self.report['phases'].values() for row in phase.get('requests', [])])
        checkpoint(self.out / 'report.json', self.report)

    def event(self, name, **values):
        event = {'event': name, 'atMonotonic': time.perf_counter(), **values}
        with (self.out / 'events.jsonl').open('a') as handle:
            handle.write(json.dumps(event) + '\n')
        print(json.dumps(event), flush=True)

    def journal(self, event, **values):
        with (self.out / 'requests.jsonl').open('a') as handle:
            handle.write(json.dumps({'event': event, 'atMonotonic': time.perf_counter(), 'phase': self.current, **values}) + '\n')

    async def observe_phases(self):
        while self.process.returncode is None:
            service = shared.usage(self.process.pid)
            client = shared.usage(os.getpid())
            if service and client:
                values = self.report.setdefault('phaseProcessMemoryPeaks', {}).setdefault(shared.CURRENT_PHASE, {'serviceRssBytes': 0, 'servicePhysicalBytes': 0 if shared.USAGE_API else None, 'generatorRssBytes': 0})
                values['serviceRssBytes'] = max(values['serviceRssBytes'], service['rss'])
                values['generatorRssBytes'] = max(values['generatorRssBytes'], client['rss'])
                if shared.USAGE_API:
                    values['servicePhysicalBytes'] = max(values['servicePhysicalBytes'], service['footprint'])
            await asyncio.sleep(.02)

    async def call(self, tool, arguments=None, agent=0, write=False):
        value = await self.rpc(tool, arguments, agent, write)
        if write and self.args.persistence == 'save-after-each-write':
            await self.rpc('save_document', {'filePath': str(self.document)}, agent)
        return value

    async def rpc(self, tool, arguments=None, agent=0, write=False):
        self.serial += 1
        identifier = f'{self.current}-{agent}-{self.serial}'
        arguments = {**(arguments or {}), 'agentId': f'real-agent-{agent}'}
        started = time.perf_counter()
        self.uncertain[identifier] = {'tool': tool, 'write': write, 'agent': agent}
        row = {'tool': tool, 'write': write, 'agent': agent, 'id': identifier}
        self.journal('request-start', **row)
        try:
            wire = await shared.request(self.port, 'POST', '/mcp', {
                'jsonrpc': '2.0', 'id': identifier, 'method': 'tools/call',
                'params': {'name': tool, 'arguments': arguments}})
            self.uncertain.pop(identifier)
            if wire.get('id') != identifier or wire.get('error'):
                raise Rejected(str(wire)[:1200])
            result = wire.get('result', {})
            text = result.get('content', [{}])[0].get('text', '')
            if result.get('isError'):
                raise Rejected(text[:1200])
            value = json.loads(text) if text else {}
            row['accepted'] = True
            if tool == 'set_active_page':
                index = int(arguments['index'])
                assert 0 <= index < self.fixture['pageCount']
                self.accepted_active_pages[agent] = index
                row['acceptedActivePageIndex'] = index
            return value
        except Rejected as error:
            row.update(accepted=False, rejected=str(error))
            raise
        except BaseException as error:
            if isinstance(error, RuntimeError) and str(error).startswith(('HTTP429:', 'HTTP503:')):
                self.uncertain.pop(identifier, None)
                row.update(accepted=False, rejected=str(error), resourceRefusal=True)
                raise Rejected(str(error)) from error
            row.update(accepted=False, transportOrUncertainError=f'{type(error).__name__}: {error}')
            raise
        finally:
            row['latencySeconds'] = time.perf_counter() - started
            self.journal('request-end', **row)
            if self.current:
                self.report['phases'][self.current]['requests'].append(row)

    async def node(self, target, agent):
        value = await self.call('read_nodes', {'pageId': target['pageId'],
            'nodeIds': [target['id']], 'depth': 0}, agent)
        assert len(value['nodes']) == 1 and value['nodes'][0]['id'] == target['id'], value
        return value['nodes'][0]

    async def page(self, target, agent):
        await self.call('set_active_page', {'index': str(target['pageIndex'])}, agent)

    async def patch(self, target, values, agent):
        result = await self.call('update_node', {'node_id': target['id'],
            'pageId': target['pageId'], 'data': values}, agent, True)
        assert str(result.get('wrote')).lower() == 'true', result
        self.expected.setdefault(target['id'], {}).update(values)
        actual = await self.node(target, agent)
        for key, value in values.items():
            assert actual.get(key) == value, (target['id'], key, value, actual.get(key))

    async def selected_roots(self, agent):
        value = await self.call('get_selection_set', {}, agent)
        ids = [value for value in value.get('ids', '').split(',') if value]
        assert ids, value
        return ids

    async def erase(self, identifier, target, agent):
        await self.call('delete_node', {'node_id': identifier,
                                      'pageId': target['pageId']}, agent, True)

    async def raster(self, identifier, agent, label):
        value = await self.call('export_item', {'itemId': identifier,
            'format': 'png', 'scale': '1'}, agent)
        data = base64.b64decode(value['bytes_base64'], validate=True)
        assert data[:8] == b'\x89PNG\r\n\x1a\n'
        width, height = struct.unpack('>II', data[16:24])
        assert 0 < width <= self.args.max_raster_edge and 0 < height <= self.args.max_raster_edge
        (self.out / f'{label}-{agent}.png').write_bytes(data)
        return {'width': width, 'height': height, 'sha256': hashlib.sha256(data).hexdigest(),
                'centerPixel': pixels.center_pixel(data)}

    async def workflow(self, family, target, agent):
        if family in families.FAMILIES:
            return await self.extra_workflow(family, target, agent)
        identifier = target['id']
        token = f'real-bench-{family}-{agent}'
        if family == 'read':
            await self.node(target, agent)
        elif family == 'search':
            value = await self.call('batch_get', {'nodeIds': [identifier],
                'pageId': target['pageId'], 'readDepth': 0}, agent)
            assert [node['id'] for node in value['nodes']] == [identifier]
        elif family == 'move':
            await self.patch(target, {'x': agent + 17, 'y': agent + 23}, agent)
        elif family == 'resize':
            await self.patch(target, {'width': 32 + agent % 9, 'height': 28 + agent % 7}, agent)
        elif family == 'text':
            await self.page(target, agent)
            await self.call('set_node_text', {'node_id': identifier, 'text': token}, agent, True)
            self.expected.setdefault(identifier, {})['content'] = token
            assert (await self.node(target, agent))['content'] == token
        elif family == 'fill':
            await self.patch(target, {'fill': [{'type': 'solid', 'color': '#2563eb', 'opacity': 1.0}]}, agent)
        elif family == 'duplicate':
            await self.call('copy_node', {'node_id': identifier, 'parent': target.get('parentId') or '',
                'pageId': target['pageId'], 'overrides': {'name': token}}, agent, True)
            value = await self.call('batch_get', {'pageId': target['pageId'],
                'patterns': [{'name': '^' + token + '$'}], 'readDepth': 0}, agent)
            assert len(value['nodes']) == 1 and value['nodes'][0]['id'] != identifier, value
            await self.erase(value['nodes'][0]['id'], target, agent)
        elif family == 'shadow':
            await self.page(target, agent)
            before = await self.node(target, agent)
            assert 'effects' in before and isinstance(before['effects'], list)
            index = len(before['effects'])
            await self.call('add_node_effect', {'node_id': identifier, 'kind': 'shadow'}, agent, True)
            assert len((await self.node(target, agent))['effects']) == index + 1
            await self.call('remove_node_effect', {'node_id': identifier, 'index': str(index)}, agent, True)
            assert (await self.node(target, agent))['effects'] == before['effects']
        elif family == 'layout':
            value = await self.call('snapshot_layout', {'pageId': target['pageId'],
                'parentId': identifier, 'maxDepth': 0}, agent)
            assert isinstance(value.get('layout'), list), value
            for node in value['layout']:
                assert all(math.isfinite(float(node[key])) for key in ('x', 'y', 'width', 'height'))
        elif family in ('image-export', 'node-export'):
            await self.page(target, agent)
            result = await self.raster(identifier, agent, family)
            self.report['phases'][self.current].setdefault('pngs', []).append({'nodeId': identifier, **result})
        elif family == 'selection-zoom':
            await self.page(target, agent)
            await self.call('set_selection', {'node_id': identifier}, agent)
            zoom = 110 + agent % 400
            await self.call('set_viewport', {'zoom_percent': str(zoom)}, agent)
            selected = await self.call('get_selection_set', {}, agent)
            viewport = await self.call('get_viewport', {}, agent)
            assert selected['anchor'] == identifier and selected['ids'] == identifier, selected
            assert int(viewport['zoom_percent']) == zoom, viewport
        elif family == 'clipboard':
            await self.page(target, agent)
            await self.call('set_selection', {'node_id': identifier}, agent)
            await self.call('copy_selected', {}, agent)
            await self.call('paste_clipboard', {'offset_px': '0'}, agent, True)
            copied = await self.selected_roots(agent)
            assert len(copied) == 1 and copied[0] != identifier
            actual = await self.node({**target, 'id': copied[0]}, agent)
            assert actual['type'] == target['type']
            await self.erase(copied[0], target, agent)
        elif family == 'undo-redo':
            await self.page(target, agent)
            before = await self.node(target, agent)
            await self.call('update_node', {'node_id': identifier, 'name': token,
                'pageId': target['pageId']}, agent, True)
            await self.call('undo', {}, agent, True)
            assert (await self.node(target, agent)).get('name') == before.get('name')
            await self.call('redo', {}, agent, True)
            assert (await self.node(target, agent))['name'] == token
            await self.call('undo', {}, agent, True)
            assert (await self.node(target, agent)).get('name') == before.get('name')
        elif family == 'svg-import-export':
            await self.page(target, agent)
            svg = '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><circle cx="8" cy="8" r="8" fill="#00aa55"/></svg>'
            await self.call('import_svg', {'svg': svg, 'x': '0', 'y': '0'}, agent, True)
            created = await self.selected_roots(agent)
            assert len(created) == 1
            result = await self.raster(created[0], agent, 'svg')
            assert result['centerPixel'] == [0, 170, 85], result
            await self.erase(created[0], target, agent)
        elif family == 'component':
            await self.page(target, agent)
            before = await self.node(target, agent)
            assert not before.get('reusable'), 'Do not replace an existing component definition'
            await self.call('create_component', {'node_id': identifier, 'name': token}, agent, True)
            await self.call('instantiate_component', {'component_id': identifier}, agent, True)
            created = await self.selected_roots(agent)
            assert len(created) == 1 and created[0] != identifier
            await self.erase(created[0], target, agent)
            await self.call('delete_component', {'component_id': identifier}, agent, True)
            if before.get('reusable') is False:
                await self.patch(target, {'reusable': False}, agent)
            actual = await self.node(target, agent)
            assert actual.get('reusable') == before.get('reusable')
            assert ('reusable' in actual) == ('reusable' in before)
            # delete_component clears to None; explicitly restore an authored false.
        else:
            raise AssertionError('Unhandled implemented family: ' + family)

    async def phase(self, spec):
        family = spec['family']
        missing = sorted(set(spec.get('tools', [])) - self.catalog)
        if missing:
            self.report['coverage'][family] = {'status': 'mcp-missing', 'missingTools': missing}
            return
        if spec.get('execution') != 'implemented':
            matches = sorted(name for name in self.catalog if any(word in name for word in spec.get('catalogTerms', [])))
            self.report['coverage'][family] = {'status': 'available-not-benchmarked' if matches else 'no-matching-mcp-tool',
                'reason': spec['reason'], 'catalogMatches': matches}
            return
        metadata = spec['role'] == 'document-metadata'
        targets = [self.fixture] if metadata else self.fixture['worksets'].get(spec['role'], [])
        if spec['family'] in ('image-export', 'node-export'):
            targets = [node for node in targets if all(isinstance(node.get(key), (int, float)) and 0 < node[key] <= self.args.max_raster_edge for key in ('width', 'height'))]
        if not targets:
            self.report['coverage'][family] = {'status': 'fixture-workset-missing', 'role': spec['role']}
            return
        count, family_budget = plan.admission(spec, self.clients, self.clients if metadata else len(targets), self.args.render_budget,
            self.args.write_budget_scope, self.args.write_budget, self.writes, self.args.family_write_budget)
        if not count:
            self.report['coverage'][family] = {'status': 'budget-deferred', 'logicalClients': self.clients,
                                              'attemptedClients': 0, 'writeRequestsSoFar': self.writes}
            return
        self.writes += count * spec.get('writes', 0)
        self.current = family
        shared.phase(family)
        self.event('phase-start', phase=family, serverPid=self.process.pid)
        row = {'logicalClients': self.clients, 'attemptedClients': 0,
            'distinctTargets': min(count, len(targets)), 'distinctTargetsAvailable': len(targets), 'targetRole': spec['role'],
            'targetSelectionPolicy': ('One full-document metadata authority; each writer owns distinct variable/axis names' if metadata else 'Distinct original targets for mutations; read-only clients may share original targets round-robin with distinctTargets reported.'),
            'maxInflight': min(count, 1 if spec.get('serialized') else self.inflight),
            'requests': [], 'acceptedWorkflows': 0, 'rejectedWorkflows': 0,
            'scheduledClients': count, 'startedClients': 0, 'writeBudgetRequests': family_budget,
            'acceptedWorkflowAgentIds': [], 'rejectedWorkflowAgentIds': [], 'cancelledWorkflows': 0,
            'workflowSamples': [], 'correctnessFailures': [], 'peakInflight': 0}
        self.report['phases'][family] = row
        self.save()
        semaphore = asyncio.Semaphore(row['maxInflight'])
        active = 0
        start = time.perf_counter()
        cpu = shared.usage(self.process.pid)

        async def client(agent):
            nonlocal active
            async with semaphore:
                active += 1
                row['startedClients'] += 1
                row['attemptedClients'] = row['startedClients']
                self.event('workflow-start', phase=family, agent=agent)
                row['peakInflight'] = max(row['peakInflight'], active)
                began = time.perf_counter()
                try:
                    await self.workflow(family, targets[agent % len(targets)], agent)
                    row['acceptedWorkflows'] += 1
                    row['acceptedWorkflowAgentIds'].append(agent)
                except Rejected as error:
                    row['rejectedWorkflows'] += 1
                    row['rejectedWorkflowAgentIds'].append(agent)
                    row.setdefault('rejections', []).append({'agent': agent, 'message': str(error)})
                except asyncio.CancelledError:
                    row['cancelledWorkflows'] += 1
                    raise
                except Exception as error:
                    row['correctnessFailures'].append({'agent': agent, 'error': f'{type(error).__name__}: {error}'})
                    raise
                finally:
                    active -= 1
                    row['workflowSamples'].append(time.perf_counter() - began)
        tasks = [asyncio.create_task(client(agent)) for agent in range(count)]
        try:
            await asyncio.gather(*tasks)
        except BaseException:
            for task in tasks:
                if not task.done():
                    task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
            raise
        finally:
            after = shared.usage(self.process.pid)
            row['elapsedSeconds'] = time.perf_counter() - start
            row['serviceCpuSeconds'] = after['cpuSeconds'] - cpu['cpuSeconds'] if cpu and after else None
            for label, samples in [('workflow', row['workflowSamples']),
                                   ('request', [x['latencySeconds'] for x in row['requests']])]:
                if samples:
                    row[label + 'LatencyP50Seconds'] = shared.percentile(samples, .5)
                    row[label + 'LatencyP95Seconds'] = shared.percentile(samples, .95)
            row['acceptedRequests'] = sum(x.get('accepted', False) for x in row['requests'])
            row['rejectedRequests'] = sum('rejected' in x for x in row['requests'])
            row['unconfirmedRequests'] = sum('transportOrUncertainError' in x for x in row['requests'])
            row.update(plan.participation(row['requests']))
            row['distinctTargets'] = min(row['startedClients'], len(targets))
            self.report['coverage'][family] = {'status': plan.completion(row, self.clients),
                'attemptedClients': row['startedClients'], 'scheduledClients': count, 'logicalClients': self.clients,
                'acceptedWorkflows': row['acceptedWorkflows'], 'rejectedWorkflows': row['rejectedWorkflows']}
            self.save()

    def verify_disk(self, source, destination):
        # After the owned server stops: retain only one complete parsed tree
        # at a time and compare all fields, not a whitelist of visible nodes.
        self.event('verification-start', phase='durable-canonical')
        expected = json.loads(source.read_text())
        before_nodes = list(shared.nodes(expected))
        ids = [node['id'] for node in before_nodes]
        assert len(ids) == len(set(ids)) == self.fixture['nodeCount']
        source_ids_hash = semantic_hash(sorted(ids))
        found = set()
        for node in before_nodes:
            if node['id'] in self.expected:
                node.update(self.expected[node['id']])
                found.add(node['id'])
        assert found == self.expected.keys()
        assert self.saved_active_page is not None, 'No accepted save actor page observation'
        expect_saved_active_page(expected, self.saved_active_page)
        expected_hash = semantic_hash(expected)
        del expected, before_nodes, ids
        gc.collect()
        actual = json.loads(destination.read_text())
        actual_nodes = list(shared.nodes(actual))
        actual_ids = [node['id'] for node in actual_nodes]
        assert len(actual_ids) == len(set(actual_ids)) == self.fixture['nodeCount']
        assert semantic_hash(sorted(actual_ids)) == source_ids_hash
        actual_hash = semantic_hash(actual)
        result = {'nodes': len(actual_ids), 'uniqueIds': len(set(actual_ids)),
            'expectedWholeDocumentSemanticSha256': expected_hash,
            'actualWholeDocumentSemanticSha256': actual_hash,
            'exactWholeDocumentSemantics': actual_hash == expected_hash,
            'scope': 'Every field/array and numeric f64 bits, including images and unknown retained JSON. Object key order is normalized; no field dropping.',
            'originalNodeIdsUnchanged': True, 'expectedMutatedNodes': len(self.expected),
            'expectedEditorMetaActivePageIndex': self.saved_active_page,
            'editorMetadataRule': 'Only activePageIndex is changed to the accepted save actor observation; all existing editorMeta fields remain. If absent, the documented save extension adds default preserveAuthoredGeometry=false.'}
        self.report['durability'] = result
        assert result['exactWholeDocumentSemantics'], 'Unexpected full-document canonical change; retained for diagnosis'

    async def run(self):
        self.out.mkdir(parents=True, exist_ok=False)
        shutil.copyfile(Path(__file__), self.out / 'harness.py')
        shutil.copyfile(Path(__file__).with_suffix('.json'), self.out / 'coverage.json')
        shutil.copyfile(Path(__file__).with_name('real-document-families.py'), self.out / 'families.py')
        shutil.copyfile(Path(__file__).with_name('real-document-plan.py'), self.out / 'plan.py')
        self.report['familyWorkflowSha256'] = shared.sha(self.out / 'families.py')
        self.report['admissionPlanSha256'] = shared.sha(self.out / 'plan.py')
        self.report['harnessSha256'] = shared.sha(self.out / 'harness.py')
        self.report['coverageManifestSha256'] = shared.sha(self.out / 'coverage.json')
        source = Path(self.fixture['document']['path']).resolve()
        assert shared.sha(source) == self.fixture['document']['sha256']
        document = self.out / 'document.op'
        self.document = document
        shutil.copyfile(source, document)
        assert shared.sha(document) == self.fixture['document']['sha256']
        environment = os.environ.copy()
        environment.pop('OPENPENCIL_FONT_FILES', None)
        font_contract = self.args.font_contracts.get(self.fixture['id'])
        if font_contract:
            assert font_contract['document']['sha256'] == self.fixture['document']['sha256']
            assert len(font_contract['files']) <= 64
            assert sum(Path(row['path']).stat().st_size for row in font_contract['files']) <= 64 * 1024 * 1024
            for row in font_contract['files']:
                assert shared.sha(Path(row['path'])) == row['sha256']
            environment.update(font_contract['environment'])
            self.report['configuredFontContract'] = font_contract
            self.report['runtimeFontIdentityDirectlyObserved'] = False
        else:
            self.report['configuredFontContract'] = None
            self.report['renderResourceCondition'] = 'No startup imported font environment; fallback-only pin11 baseline is not authored-font parity.'
        command = [str(self.args.binary), '--serve-web', '--managed', '--port', '0',
            '--file', str(document), '--config-root', str(self.out / 'config')]
        self.report.update(command=command, copiedWholeSourceBytes=source.stat().st_size)
        self.save()
        start = time.perf_counter()
        before = resource.getrusage(resource.RUSAGE_CHILDREN)
        log = (self.out / 'server.stderr.log').open('wb')
        try:
            shared.phase('startup')
            self.process = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=log, env=environment)
            self.report['serverPid'] = self.process.pid
            self.event('server-start', serverPid=self.process.pid)
            self.save()
            self.sampler = asyncio.create_task(shared.monitor(self.process, self.report, self.args.max_bytes))
            self.observer = asyncio.create_task(self.observe_phases())
            handshake = json.loads(await asyncio.wait_for(self.process.stdout.readline(), self.args.startup_timeout))
            assert handshake.get('ok'), handshake
            self.port = handshake['port']
            self.report['startupSeconds'] = time.perf_counter() - start
            wire = await shared.request(self.port, 'POST', '/mcp', {
                'jsonrpc': '2.0', 'id': 'catalog', 'method': 'tools/list'})
            tools = wire['result']['tools']
            checkpoint(self.out / 'live-tools-list.json', wire)
            self.catalog = {tool['name'] for tool in tools}
            for spec in self.manifest['families']:
                self.report['coverage'][spec['family']] = {'status': 'not-attempted'}
            self.current = 'startup-invariants'
            shared.phase(self.current)
            self.report['phases'][self.current] = {'requests': []}
            info = await self.call('get_document_info')
            assert int(info['page_count']) == self.fixture['pageCount']
            assert int(info['total_nodes']) == self.fixture['nodeCount']
            self.report['loadedFullDocumentInfo'] = info
            self.save()
            self.report['liveCatalog'] = {'toolCount': len(tools),
                'sha256': shared.sha(self.out / 'live-tools-list.json')}
            if self.args.profile_rendezvous:
                self.event('profile-rendezvous', serverPid=self.process.pid, seconds=self.args.profile_rendezvous)
                await asyncio.sleep(self.args.profile_rendezvous)
                self.report['profileRendezvousSecondsExcludedFromOperationTiming'] = self.args.profile_rendezvous
            for spec in self.manifest['families']:
                if self.args.family and spec['family'] not in self.args.family:
                    self.report['coverage'][spec['family']] = {'status': 'not-requested'}
                else:
                    await self.phase(spec)
            assigned = {tool for row in self.manifest['families'] for tool in row.get('tools', [])}
            self.report['liveToolsNotExercisedByManifest'] = sorted(self.catalog - assigned)
            if self.args.readonly_no_save:
                assert not any(row.get('write') for phase in self.report['phases'].values() for row in phase.get('requests', []))
                self.report['saveScope'] = 'Read-only diagnostic: no save request; not a durability-write benchmark.'
            else:
                self.current = 'save'
                shared.phase('save')
                self.report['phases']['save'] = {'requests': []}
                observed = await self.call('get_editor_state', {}, agent=0)
                active_page = int(observed['active_page_index'])
                assert 0 <= active_page < self.fixture['pageCount']
                assert self.accepted_active_pages.get(0, active_page) == active_page
                self.report['saveActorPageObservation'] = {
                    'agent': 0, 'activePageIndex': active_page,
                    'lastAcceptedSetActivePage': self.accepted_active_pages.get(0),
                    'source': 'accepted get_editor_state immediately before final save; same agent context'}
                saved = await self.call('save_document', {'filePath': str(document)}, write=False)
                self.saved_active_page = active_page
                self.report['saveResponse'] = saved
            self.report['status'] = 'operations-completed'
        except asyncio.CancelledError:
            self.report.update(status='deadline', error=f'Case deadline{self.args.timeout}s; in-flight writes are not replayed')
        except BaseException as error:
            self.report.update(status='blocked-resource' if self.report.get('memoryBudgetExceeded') else 'failed',
                               error=f'{type(error).__name__}: {error}')
        finally:
            self.report['unconfirmedInFlightRequests'] = list(self.uncertain.values())
            self.save()
            if self.process:
                if self.process.stdin:
                    self.process.stdin.close()
                try:
                    await asyncio.wait_for(self.process.wait(), 5)
                except asyncio.TimeoutError:
                    self.process.terminate()
                    await self.process.wait()
                self.report['serverExitCode'] = self.process.returncode
            if self.sampler:
                await self.sampler
            if self.observer:
                await self.observer
            log.close()
            after = resource.getrusage(resource.RUSAGE_CHILDREN)
            self.report.update(totalWallSeconds=time.perf_counter() - start,
                totalServiceCpuSeconds=after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime,
                reservedMutatingRequests=self.writes,
                sourceUnchanged=shared.sha(source) == self.fixture['document']['sha256'])
            if self.report.get('memoryBudgetExceeded'):
                self.report['status'] = 'blocked-resource'
            self.save()
        if self.report['status'] == 'operations-completed':
            try:
                if self.args.readonly_no_save:
                    assert shared.sha(document) == self.fixture['document']['sha256']
                    self.report['durability'] = {'noWritesIssued': True, 'workcopyByteIdentical': True, 'writeDurabilityExercised': False}
                else:
                    self.verify_disk(source, document)
                self.report['status'] = 'passed' if self.process.returncode == 0 else 'failed'
            except BaseException as error:
                self.report.update(status='failed-correctness', error=f'{type(error).__name__}: {error}')
        phase_peaks = self.report.get('phaseProcessMemoryPeaks', {}).values()
        self.report['highestObservedServiceRssBytes'] = max([self.report.get('peakServiceRssBytes', 0), self.report.get('kernelChildMaxRssBytes', 0)] + [row['serviceRssBytes'] for row in phase_peaks])
        if shared.USAGE_API:
            self.report['highestObservedServicePhysicalBytes'] = max([self.report.get('peakServicePhysicalFootprintBytes', 0)] + [row['servicePhysicalBytes'] for row in phase_peaks])
        self.report['allRequestedClientWorkflowsAccepted'] = all(row.get('acceptedWorkflows') == row.get('logicalClients') for name, row in self.report['phases'].items() if name not in ('startup-invariants', 'save'))
        if self.report['status'] == 'passed' and any(row.get('rejectedWorkflows') for row in self.report['phases'].values()):
            self.report['status'] = 'completed-with-refusals'
        self.report['documentVersionInvariant'] = 'Not observed: current bounded MCP metadata lacks version; no whole-document fetch or extra SSE subscription was added.'
        self.report['allOperationsMeasured'] = all(x.get('status') == 'measured' for x in self.report['coverage'].values())
        self.report['strictOneGbBudgetPassed'] = self.report['status'] == 'passed' and self.args.max_bytes <= 1_000_000_000
        self.save()
        return self.report


async def main_async(args):
    args.output.mkdir(parents=True, exist_ok=False)
    build = json.loads(args.build_manifest.read_text())
    assert build['status'] == 'completed' and build['sourceStable']
    assert build['engineCommit'] == args.engine_commit
    args.binary = Path(build['outputs']['server']['path']).resolve()
    assert shared.sha(args.binary) == args.server_sha == build['outputs']['server']['sha256']
    pin = {'manifest': str(args.build_manifest.resolve()), 'engineCommit': args.engine_commit,
           'jianCommit': build['jianCommit'], 'serverSha256': args.server_sha}
    manifest = plan.manifest(HERE.with_suffix('.json'))
    args.font_contracts = {value['fixture']: value for path in (args.font_environment or []) for value in [json.loads(path.read_text())]}
    fixtures = []
    for path in args.fixture:
        fixture = json.loads(path.read_text())
        fixture['fixtureSource'] = str(path.resolve())
        fixture['fixtureSha256'] = shared.sha(path)
        by_id = {node['id']: node for nodes in fixture['worksets'].values() for node in nodes if node.get('effectsCount', 0) > 0}
        fixture['worksets']['effect'] = list(by_id.values())
        fixtures.append(fixture)
    reports = []
    for fixture in fixtures:
        startup_block = None
        for clients in args.clients:
            active = clients if args.requested_count_stress else min(clients, args.max_inflight)
            out = args.output / fixture['id'] / str(clients)
            out.parent.mkdir(exist_ok=True)
            if startup_block:
                out.mkdir()
                report = {'status': 'not-run-startup-prerequisite', 'logicalClients': clients, 'attemptedClients': 0,
                    'requestedActiveConcurrency': active, 'actualActiveConcurrency': 0, 'binaryPin': pin,
                    'fixture': fixture, 'reason': 'Same full-document cold startup exceeded this exact binary/fixture memory allowance before any operation.',
                    'blockingReport': startup_block, 'allOperationsMeasured': False}
                checkpoint(out / 'report.json', report)
                reports.append({'fixture': fixture['id'], 'clients': clients, 'status': report['status'], 'report': str(out / 'report.json')})
                checkpoint(args.output / 'summary.json', {'results': reports, 'pin': pin, 'allOpsClaim': False})
                continue
            case = Case(args, fixture, clients, active, out, manifest, pin)
            task = asyncio.create_task(case.run())
            try:
                report = await asyncio.wait_for(task, args.timeout)
            except asyncio.TimeoutError:
                report = case.report
                report.update(status='deadline', error=f'Case deadline{args.timeout}s; no write retry')
                case.save()
            if report['status'] == 'blocked-resource' and report.get('startupSeconds') is None:
                startup_block = str(out / 'report.json')
            reports.append({'fixture': fixture['id'], 'clients': clients,
                'status': report['status'], 'report': str(out / 'report.json')})
            checkpoint(args.output / 'summary.json', {'results': reports, 'pin': pin,
                'allOpsClaim': False, 'coverageManifest': str(Path(__file__).with_suffix('.json'))})
    return reports


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    family_choices = [row['family'] for row in plan.manifest(HERE.with_suffix('.json'))['families']]
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--engine-commit', required=True)
    parser.add_argument('--server-sha', required=True)
    parser.add_argument('--font-environment', type=Path, nargs='+')
    parser.add_argument('--persistence', choices=['live-final-save', 'save-after-each-write'], default='live-final-save')
    parser.add_argument('--managed-primary-save-before-ack', action='store_true', help='Label a pinned service whose Managed bound command writes save before success; not a request to change service behavior')
    parser.add_argument('--fixture', type=Path, nargs='+', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--clients', type=int, nargs='+', default=[1, 10, 100, 1000])
    parser.add_argument('--max-inflight', type=int, default=32)
    parser.add_argument('--requested-count-stress', action='store_true')
    parser.add_argument('--family', nargs='+', choices=family_choices, help='Select bounded named families; unselected families remain explicitly not-requested')
    parser.add_argument('--write-budget', type=int, help='Global mutation request cap (default64); unavailable in family mode')
    parser.add_argument('--write-budget-scope', choices=['family', 'global'], default='global')
    parser.add_argument('--family-write-budget', type=int, help='Per-family mutation request cap; default is enough to schedule every logical writer for its whole workflow')
    parser.add_argument('--render-budget', type=int, default=4)
    parser.add_argument('--max-raster-edge', type=int, default=1024)
    parser.add_argument('--max-bytes', type=int, default=1_000_000_000)
    parser.add_argument('--timeout', type=float, default=90)
    parser.add_argument('--readonly-no-save', action='store_true')
    parser.add_argument('--profile-rendezvous', type=float, default=0)
    parser.add_argument('--startup-timeout', type=float, default=45)
    args = parser.parse_args()
    if args.write_budget_scope == 'family' and args.write_budget is not None:
        parser.error('--write-budget is global; use --family-write-budget in family mode')
    if args.write_budget_scope == 'global' and args.family_write_budget is not None:
        parser.error('--family-write-budget requires --write-budget-scope family')
    args.write_budget = 64 if args.write_budget is None else args.write_budget
    assert all(1 <= n <= 1000 for n in args.clients) and 1 <= args.max_inflight <= 1000
    assert 1 <= args.max_bytes <= 5_000_000_000 and 1 <= args.timeout <= 180
    assert not args.readonly_no_save or (args.write_budget_scope == 'global' and args.write_budget == 0) or (args.write_budget_scope == 'family' and args.family_write_budget == 0)
    assert 0 <= args.profile_rendezvous <= 5
    assert 0 <= args.write_budget <= 10000 and 0 <= args.render_budget <= 1000
    assert args.family_write_budget is None or 0 <= args.family_write_budget <= 100000
    args.output = args.output.resolve()
    reports = asyncio.run(main_async(args))
    sys.exit(0 if all(row['status'] == 'passed' for row in reports) else 1)


if __name__ == '__main__':
    main()
