"""Additional real-document workflows. No workload runs on import."""
import hashlib
import json
import re
import struct

FAMILIES = {'reparent', 'reorder', 'group-ungroup', 'group-ungroup-nested', 'align', 'variables', 'themes'}


class FamilyWorkflows:
    def owned_name(self, family, agent, suffix):
        run = hashlib.sha256(str(self.out.resolve()).encode()).hexdigest()[:12]
        return f'real-owned-{run}-{family}-{agent}-{suffix}'

    async def named(self, name, target, agent, absent=False):
        result = await self.call('batch_get', {'pageId': target['pageId'],
            'patterns': [{'name': '^' + re.escape(name) + '$'}], 'readDepth': 0}, agent)
        nodes = result['nodes']
        assert len(nodes) == (0 if absent else 1), (name, nodes)
        return None if absent else nodes[0]['id']

    async def owned_frame(self, family, target, agent, suffix):
        name = self.owned_name(family, agent, suffix)
        await self.named(name, target, agent, absent=True)
        await self.call('insert_node', {'kind': 'frame', 'name': name, 'x': '100', 'y': '100',
            'width': '800', 'height': '600', 'parent': '', 'pageId': target['pageId']}, agent, True)
        identifier = await self.named(name, target, agent)
        self.event('owned-target', phase=self.current, agent=agent, nodeId=identifier,
            role='temporary-frame')
        return identifier

    async def owned_copies(self, family, target, agent, parent=''):
        copies = []
        for index in range(3):
            name = self.owned_name(family, agent, f'copy-{index}')
            await self.named(name, target, agent, absent=True)
            await self.call('copy_node', {'node_id': target['id'], 'parent': parent,
                'pageId': target['pageId'], 'overrides': {'name': name, 'x': 12 + 29 * index,
                    'y': 14 + 23 * index, 'width': 16, 'height': 12, 'rotation': 0,
                    'enabled': True, 'locked': False}}, agent, True)
            copied = await self.named(name, target, agent)
            assert copied != target['id'] and copied not in copies
            copies.append(copied)
            self.event('owned-target', phase=self.current, agent=agent,
                nodeId=copied, sourceNodeId=target['id'])
        self.report['phases'][self.current].setdefault('ownedImportedCopyIds', []).append(
            {'agent': agent, 'sourceNodeId': target['id'], 'copies': copies})
        return copies

    async def owned_children(self, parent, target, agent):
        value = await self.call('read_nodes', {'pageId': target['pageId'], 'nodeIds': [parent], 'depth': 1}, agent)
        assert len(value['nodes']) == 1
        return value['nodes'][0].get('children', [])

    async def cleanup_owned(self, copies, frames, target, agent):
        # Children first: never issue a delete for a node its parent already removed.
        for identifier in [*copies, *reversed(frames)]:
            await self.erase(identifier, target, agent)
        value = await self.call('read_nodes', {'pageId': target['pageId'],
            'nodeIds': [*copies, *frames], 'depth': 0}, agent)
        assert value['nodes'] == []
        self.report['phases'][self.current].setdefault('cleanedAgents', []).append(agent)

    async def extra_workflow(self, family, target, agent):
        await getattr(self, 'family_' + family.replace('-', '_'))(target, agent)

    async def family_reparent(self, target, agent):
        await self.page(target, agent)
        frames = [await self.owned_frame('reparent', target, agent, side) for side in ('a', 'b')]
        copies = await self.owned_copies('reparent', target, agent, frames[0])
        for identifier in copies[:2]:
            await self.call('move_node', {'node_id': identifier, 'parent': frames[1],
                'index': '0', 'pageId': target['pageId']}, agent, True)
        assert [n['id'] for n in await self.owned_children(frames[1], target, agent)] == copies[1::-1]
        await self.call('move_node', {'node_id': copies[0], 'parent': frames[0],
            'index': '1', 'pageId': target['pageId']}, agent, True)
        assert [n['id'] for n in await self.owned_children(frames[0], target, agent)] == [copies[2], copies[0]]
        await self.cleanup_owned(copies, frames, target, agent)

    async def family_reorder(self, target, agent):
        await self.page(target, agent)
        frame = await self.owned_frame('reorder', target, agent, 'parent')
        copies = await self.owned_copies('reorder', target, agent, frame)
        await self.call('set_selection', {'node_id': copies[1]}, agent)
        await self.call('reorder_selected', {'direction': 'up'}, agent, True)
        assert [n['id'] for n in await self.owned_children(frame, target, agent)] == [copies[1], copies[0], copies[2]]
        await self.call('reorder_selected', {'direction': 'down'}, agent, True)
        assert [n['id'] for n in await self.owned_children(frame, target, agent)] == copies
        await self.cleanup_owned(copies, [frame], target, agent)

    async def family_group_ungroup(self, target, agent):
        await self.page(target, agent)
        copies = await self.owned_copies('group', target, agent)
        await self.group_roundtrip(copies, target, agent)
        await self.cleanup_owned(copies, [], target, agent)

    async def family_group_ungroup_nested(self, target, agent):
        await self.page(target, agent)
        frame = await self.owned_frame('nested-group', target, agent, 'parent')
        copies = await self.owned_copies('nested-group', target, agent, frame)
        await self.group_roundtrip(copies, target, agent)
        assert [n['id'] for n in await self.owned_children(frame, target, agent)] == copies
        await self.cleanup_owned(copies, [frame], target, agent)

    async def group_roundtrip(self, copies, target, agent):
        await self.call('set_selection_set', {'node_ids': ','.join(copies)}, agent)
        await self.call('group_selected', {}, agent, True)
        grouped = await self.selected_roots(agent)
        assert len(grouped) == 1 and grouped[0] not in copies
        assert [n['id'] for n in await self.owned_children(grouped[0], target, agent)] == copies
        await self.call('ungroup_selected', {}, agent, True)
        assert await self.selected_roots(agent) == copies
        value = await self.call('read_nodes', {'pageId': target['pageId'], 'nodeIds': grouped, 'depth': 0}, agent)
        assert value['nodes'] == []

    async def family_align(self, target, agent):
        await self.page(target, agent)
        frame = await self.owned_frame('align', target, agent, 'parent')
        copies = await self.owned_copies('align', target, agent, frame)
        await self.call('set_selection_set', {'node_ids': ','.join(copies)}, agent)
        for action, field, expected in [('left', 'x', 12), ('top', 'y', 14)]:
            await self.call('align_selected', {'action': action}, agent, True)
            nodes = await self.owned_children(frame, target, agent)
            assert [n['id'] for n in nodes] == copies
            assert all(n[field] == expected for n in nodes), (action, nodes)
        await self.cleanup_owned(copies, [frame], target, agent)

    async def family_variables(self, target, agent):
        # Global undo is shared. Serialize complete workflows; never undo a
        # different agent's token or restore all maps with a racing replace.
        before = await self.call('get_variables', {}, agent)
        definitions = json.loads(before['variables'])
        cases = [('number', '0.125', '-0.0', -0.0, 'set_variable_number', 'value'),
                 ('color', '#112233', '#2563eb', '#2563eb', 'set_variable_color', 'hex'),
                 ('boolean', 'false', 'true', True, 'set_variable_boolean', 'value'),
                 ('string', 'before', 'after "\\\nΩ', 'after "\\\nΩ', 'set_variable_string', 'value')]
        for kind, default, changed, expected, setter, field in cases:
            name = self.owned_name('variable', agent, kind)
            assert name not in definitions
            await self.call('create_variable', {'name': name, 'kind': kind, 'default_value': default}, agent, True)
            await self.call(setter, {'name': name, field: changed}, agent, True)
            current = await self.call('get_variables', {}, agent)
            value = json.loads(current['variables'])[name]
            assert value == {'type': kind, 'value': expected}, value
            if kind == 'number':
                assert struct.pack('>d', value['value']) == struct.pack('>d', expected)
            await self.call('delete_variable', {'name': name}, agent, True)
            for _ in range(3):
                await self.call('undo', {}, agent, True)
            restored = await self.call('get_variables', {}, agent)
            assert restored == before, 'restore exact maps via undo, including absent/empty in final canonical gate'

    async def family_themes(self, target, agent):
        before = await self.call('get_variables', {}, agent)
        themes = json.loads(before['themes'])
        axis = self.owned_name('theme', agent, 'axis')
        assert axis not in themes
        await self.call('set_themes', {'themes': {axis: ['Light', 'Dark']}, 'replace': False}, agent, True)
        current = await self.call('get_variables', {}, agent)
        assert json.loads(current['themes']) == {**themes, axis: ['Light', 'Dark']}
        await self.call('undo', {}, agent, True)
        assert await self.call('get_variables', {}, agent) == before
