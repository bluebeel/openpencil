"""Bounded real-file workflow admission; no service or workload starts here."""
import json
import re


def manifest(path):
    value = json.loads(path.read_text())
    rows = value['families']
    if not 1 <= len(rows) <= 64:
        raise ValueError('coverage must contain1..64families')
    names = [row['family'] for row in rows]
    if len(set(names)) != len(names) or any(not re.fullmatch(r'[a-z][a-z0-9-]{0,63}', name) for name in names):
        raise ValueError('family names must be unique bounded CLI identifiers')
    for row in rows:
        writes = row.get('writes', 0)
        if type(writes) is not int or not 0 <= writes <= 100:
            raise ValueError('workflow mutation reservations must be0..100')
    return value


def admission(spec, clients, target_count, render_budget, budget_scope,
              global_budget, reserved_so_far, family_budget):
    """Reserve whole workflows; never spend another family's independent cap."""
    count, allowance = clients, None
    writes = spec.get('writes', 0)
    if writes:
        if budget_scope == 'family':
            allowance = clients * writes if family_budget is None else family_budget
        elif budget_scope == 'global':
            allowance = max(0, global_budget - reserved_so_far)
        else:
            raise ValueError('unknown write-budget scope')
        count = min(count, target_count, allowance // writes)
    if spec.get('render'):
        count = min(count, render_budget)
    return count, allowance


def completion(row, clients):
    if row['correctnessFailures']:
        return 'failed-correctness'
    if row['startedClients'] != clients or row['acceptedWorkflows'] + row['rejectedWorkflows'] != clients:
        return 'partial-budget-targets-or-deadline'
    return 'measured-with-refusals' if row['rejectedWorkflows'] else 'measured'


def participation(requests):
    accepted = [row for row in requests if row.get('accepted')]
    attempted_writes = [row for row in requests if row['write']]
    return {'acceptedMutationRequests': sum(row['write'] for row in accepted),
        'acceptedMutationParticipants': len({row['agent'] for row in accepted if row['write']}),
        'attemptedMutationParticipants': len({row['agent'] for row in attempted_writes}),
        'acceptedMutationAgentIds': sorted({row['agent'] for row in accepted if row['write']}),
        'unconfirmedMutationRequests': sum(row['write'] and 'transportOrUncertainError' in row for row in requests)}
