"""Select successful branch-push CI at the exact release SHA, never PR merge builds."""
from datetime import datetime, timezone
import json
import os
import subprocess
import time

PLATFORMS = ('linux-x64', 'windows-x64', 'macos-arm64')


def api(path):
    pages = json.loads(subprocess.check_output(['gh', 'api', '--paginate', '--slurp', path], text=True))
    return pages


def select_run(pages, repo, sha):
    runs = [run for page in pages for run in page['workflow_runs']
            if run['head_sha'] == sha and run['event'] == 'push'
            and run['head_repository']['full_name'] == repo]
    return max(runs, key=lambda run: run['id'], default=None)


def artifact_state(pages, now, updated):
    artifacts = [item for page in pages for item in page['artifacts']]
    expected = {f'installer-{platform}' for platform in PLATFORMS}
    selected = [item for item in artifacts if item['name'] in expected]
    if any(item['expired'] for item in selected):
        return 'rebuild'
    if {item['name'] for item in selected} != expected:
        if (now - datetime.fromisoformat(updated.replace('Z', '+00:00'))).total_seconds() >= 7 * 86400:
            return 'rebuild'
        raise ValueError('Successful package run has missing platform artifacts')
    if len(selected) != len(expected):
        raise ValueError('Duplicate platform artifacts')
    return 'reuse'


def resolve(repo, sha, timeout=3600, missing_timeout=300):
    started = time.monotonic()
    while True:
        selected = {}
        pending = False
        for workflow in ['test.yml', 'package.yml']:
            run = select_run(api(f'repos/{repo}/actions/workflows/{workflow}/runs?head_sha={sha}&event=push&per_page=100'), repo, sha)
            if run is None:
                if time.monotonic() - started >= missing_timeout:
                    raise ValueError(f'{workflow} has no branch-push run for {sha}; push the commit to a branch first')
                pending = True
                print(f'Waiting for {workflow} to appear at {sha}', flush=True)
            elif run['status'] != 'completed':
                pending = True
                print(f'Waiting for {workflow}: {run["html_url"]}', flush=True)
            elif run['conclusion'] != 'success':
                raise ValueError(f'{workflow} did not succeed: {run["conclusion"]}; {run["html_url"]}')
            else:
                selected[workflow] = run
        if not pending:
            package = selected['package.yml']
            state = artifact_state(api(f'repos/{repo}/actions/runs/{package["id"]}/artifacts?per_page=100'), datetime.now(timezone.utc), package['updated_at'])
            return str(package['id']), state == 'rebuild'
        if time.monotonic() - started >= timeout:
            raise TimeoutError('Timed out waiting for release commit CI')
        time.sleep(30)


def main():
    run_id, rebuild = resolve(os.environ['GH_REPO'], os.environ['RELEASE_SHA'])
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
        output.write(f'package_run_id={run_id}\nrebuild={str(rebuild).lower()}\n')
    print(f'Package run {run_id}: {"rebuild expired artifacts" if rebuild else "reuse verified artifacts"}')


if __name__ == '__main__':
    main()
