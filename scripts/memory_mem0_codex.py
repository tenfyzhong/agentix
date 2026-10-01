"""Codex model provider for an isolated, pinned Mem0 OSS benchmark."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from memory_reader import validate_events


class CodexMem0LLM:
    """Implements Mem0's synchronous provider interface without changing its prompts."""

    def __init__(self, config, *, output_directory=None):
        if config.model != 'gpt-6-astra' or getattr(config, 'reasoning_effort', None) != 'low':
            raise ValueError('matched benchmark requires gpt-6-astra low')
        self.config = config
        location = output_directory or os.environ.get('TASKIX_BENCH_MEM0_CALLS')
        if not location:
            raise ValueError('explicit benchmark call directory required')
        self.directory = Path(location).resolve()
        self.directory.mkdir(parents=True, exist_ok=True)

    def generate_response(self, messages, tools=None, tool_choice='auto', **kwargs):
        if tools or tool_choice != 'auto' or set(kwargs) - {'response_format'}:
            raise ValueError('unsupported Mem0 model request')
        if not messages or any(m.get('role') not in ('system', 'user', 'assistant')
                               or not isinstance(m.get('content'), str) for m in messages):
            raise ValueError('unsupported Mem0 message shape')
        response_format = kwargs.get('response_format')
        if response_format not in (None, {'type': 'json_object'}):
            raise ValueError('unsupported response format')
        directory = Path(tempfile.mkdtemp(prefix='call-', dir=self.directory))
        (directory / 'messages.json').write_text(json.dumps(messages, ensure_ascii=False, indent=2))
        instructions = '\n\n'.join(m['content'] for m in messages if m['role'] == 'system')
        (directory / 'instructions.txt').write_text(instructions or 'Respond to the supplied conversation. Use no tools.')
        payload = [m for m in messages if m['role'] != 'system']
        response = directory / 'response.json'
        command = ['codex', 'exec', '--ignore-user-config', '--ephemeral', '--skip-git-repo-check',
                   '-s', 'read-only', '-m', self.config.model, '-c', 'model_reasoning_effort="low"',
                   '-c', 'project_doc_max_bytes=0', '-c', 'features.shell_tool=false',
                   '-c', 'features.multi_agent=false', '-c', 'web_search="disabled"',
                   '-c', 'model_instructions_file=' + json.dumps(str(directory / 'instructions.txt')),
                   '--json', '--output-last-message', str(response),
                   'Respond to the following JSON conversation, preserving its message roles. Use no tools.']
        started = time.monotonic()
        try:
            with (directory / 'events.jsonl').open('w') as events, (directory / 'stderr').open('w') as errors:
                subprocess.run(command, input=json.dumps(payload, ensure_ascii=False), text=True,
                               cwd=directory, stdout=events, stderr=errors, timeout=180, check=True)
            usage = validate_events((directory / 'events.jsonl').read_text())
            answer = response.read_text()
            if response_format and not isinstance(json.loads(answer), dict):
                raise ValueError('Mem0 requires a JSON object')
            (directory / 'receipt.json').write_text(json.dumps({
                'usage': usage, 'model': self.config.model, 'reasoning': 'low',
                'latency_ms': (time.monotonic() - started) * 1000}, indent=2))
            return answer
        except Exception as error:
            (directory / 'error.txt').write_text(str(error))
            raise
