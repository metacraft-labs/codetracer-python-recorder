"""Integration test: real HTTP requests through WSGI middleware."""

import json
import os
import sys
import tempfile
import threading
import time
import unittest
import urllib.request
import urllib.error
from wsgiref.simple_server import make_server, WSGIRequestHandler

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'codetracer-python-recorder'))
from codetracer_python_recorder.middleware.wsgi import CodeTracerWSGIMiddleware


class QuietHandler(WSGIRequestHandler):
    def log_message(self, format, *args):
        pass  # Suppress server logs


def simple_app(environ, start_response):
    path = environ.get('PATH_INFO', '/')
    method = environ.get('REQUEST_METHOD', 'GET')

    if path == '/api/users' and method == 'GET':
        start_response('200 OK', [('Content-Type', 'application/json')])
        return [b'[{"id":1},{"id":2}]']
    elif path == '/api/users' and method == 'POST':
        start_response('201 Created', [('Content-Type', 'application/json')])
        return [b'{"id":3}']
    elif path == '/health':
        start_response('200 OK', [('Content-Type', 'text/plain')])
        return [b'ok']
    elif path == '/error':
        start_response('500 Internal Server Error', [('Content-Type', 'text/plain')])
        return [b'error']
    else:
        start_response('404 Not Found', [('Content-Type', 'text/plain')])
        return [b'not found']


class TestWSGIIntegration(unittest.TestCase):
    def setUp(self):
        self.manifest_fd, self.manifest_path = tempfile.mkstemp(suffix='.jsonl')
        os.close(self.manifest_fd)
        os.unlink(self.manifest_path)

        wrapped_app = CodeTracerWSGIMiddleware(simple_app, self.manifest_path)
        self.port = 18800 + os.getpid() % 100
        self.server = make_server('127.0.0.1', self.port, wrapped_app,
                                   handler_class=QuietHandler)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.daemon = True
        self.thread.start()
        time.sleep(0.3)

    def tearDown(self):
        self.server.shutdown()
        self.thread.join(timeout=5)
        if os.path.exists(self.manifest_path):
            os.unlink(self.manifest_path)

    def test_e2e_wsgi_5_requests(self):
        base = f'http://127.0.0.1:{self.port}'

        # 1. GET /api/users
        urllib.request.urlopen(f'{base}/api/users')
        # 2. POST /api/users
        req = urllib.request.Request(f'{base}/api/users', data=b'{"name":"Alice"}',
                                     headers={'Content-Type': 'application/json'},
                                     method='POST')
        urllib.request.urlopen(req)
        # 3. GET /api/users
        urllib.request.urlopen(f'{base}/api/users')
        # 4. GET /error
        try:
            urllib.request.urlopen(f'{base}/error')
        except urllib.error.HTTPError:
            pass
        # 5. GET /health
        urllib.request.urlopen(f'{base}/health')

        # Read manifest
        self.assertTrue(os.path.exists(self.manifest_path))
        with open(self.manifest_path) as f:
            lines = f.readlines()

        self.assertEqual(len(lines), 5, f"expected 5 spans, got {len(lines)}")

        spans = [json.loads(line) for line in lines]

        # Verify methods
        self.assertEqual(spans[0]['metadata']['http.method'], 'GET')
        self.assertEqual(spans[1]['metadata']['http.method'], 'POST')
        self.assertEqual(spans[2]['metadata']['http.method'], 'GET')
        self.assertEqual(spans[3]['metadata']['http.method'], 'GET')
        self.assertEqual(spans[4]['metadata']['http.method'], 'GET')

        # Verify URLs
        self.assertEqual(spans[0]['metadata']['http.url'], '/api/users')
        self.assertEqual(spans[4]['metadata']['http.url'], '/health')

        # Verify status codes
        self.assertEqual(spans[0]['metadata']['http.status_code'], '200')
        self.assertEqual(spans[1]['metadata']['http.status_code'], '201')
        self.assertEqual(spans[3]['metadata']['http.status_code'], '500')
        self.assertEqual(spans[3]['status'], 'error')

        # Verify durations
        for span in spans:
            duration = int(span['metadata']['http.duration_ms'])
            self.assertGreaterEqual(duration, 0)

        # Verify span types
        for span in spans:
            self.assertEqual(span['span_type'], 'web-request')


if __name__ == '__main__':
    unittest.main()
