"""Tests for the WSGI and ASGI middleware span tracking."""

import json
import os
import tempfile
import unittest

# Add the source to path
import sys
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'codetracer-python-recorder'))

from codetracer_python_recorder.middleware.wsgi import CodeTracerWSGIMiddleware


class SimpleWSGIApp:
    def __init__(self, status='200 OK', body=b'OK'):
        self.status = status
        self.body = body

    def __call__(self, environ, start_response):
        start_response(self.status, [('Content-Type', 'text/plain')])
        return [self.body]


class ErrorWSGIApp:
    def __call__(self, environ, start_response):
        raise RuntimeError("Test error")


class TestWSGIMiddleware(unittest.TestCase):
    def setUp(self):
        self.manifest_fd, self.manifest_path = tempfile.mkstemp(suffix='.jsonl')
        os.close(self.manifest_fd)
        os.unlink(self.manifest_path)  # Start with no file

    def tearDown(self):
        if os.path.exists(self.manifest_path):
            os.unlink(self.manifest_path)

    def make_environ(self, method='GET', path='/'):
        return {
            'REQUEST_METHOD': method,
            'PATH_INFO': path,
            'SERVER_NAME': 'localhost',
            'SERVER_PORT': '80',
            'wsgi.input': None,
        }

    def test_wsgi_middleware_emits_spans(self):
        app = CodeTracerWSGIMiddleware(SimpleWSGIApp(), self.manifest_path)

        for i in range(3):
            environ = self.make_environ('GET', f'/api/test{i}')
            captured = {}
            def start_response(status, headers, exc_info=None):
                captured['status'] = status
            app(environ, start_response)

        with open(self.manifest_path) as f:
            lines = f.readlines()

        self.assertEqual(len(lines), 3)
        for i, line in enumerate(lines):
            span = json.loads(line)
            self.assertEqual(span['metadata']['http.method'], 'GET')
            self.assertEqual(span['metadata']['http.url'], f'/api/test{i}')
            self.assertEqual(span['metadata']['http.status_code'], '200')
            self.assertEqual(span['status'], 'ok')

    def test_wsgi_post_request(self):
        app = CodeTracerWSGIMiddleware(SimpleWSGIApp('201 Created'), self.manifest_path)
        environ = self.make_environ('POST', '/users')
        app(environ, lambda s, h, e=None: None)

        with open(self.manifest_path) as f:
            span = json.loads(f.readline())

        self.assertEqual(span['metadata']['http.method'], 'POST')
        self.assertEqual(span['metadata']['http.status_code'], '201')

    def test_wsgi_error_span(self):
        app = CodeTracerWSGIMiddleware(
            SimpleWSGIApp('500 Internal Server Error'),
            self.manifest_path
        )
        environ = self.make_environ('GET', '/fail')
        app(environ, lambda s, h, e=None: None)

        with open(self.manifest_path) as f:
            span = json.loads(f.readline())

        self.assertEqual(span['status'], 'error')
        self.assertEqual(span['metadata']['http.status_code'], '500')

    def test_wsgi_exception_records_span(self):
        app = CodeTracerWSGIMiddleware(ErrorWSGIApp(), self.manifest_path)
        environ = self.make_environ('GET', '/crash')

        with self.assertRaises(RuntimeError):
            app(environ, lambda s, h, e=None: None)

        # Span should still be written
        with open(self.manifest_path) as f:
            span = json.loads(f.readline())

        self.assertEqual(span['status'], 'error')
        self.assertEqual(span['metadata']['http.status_code'], '500')

    def test_wsgi_duration_recorded(self):
        app = CodeTracerWSGIMiddleware(SimpleWSGIApp(), self.manifest_path)
        environ = self.make_environ('GET', '/')
        app(environ, lambda s, h, e=None: None)

        with open(self.manifest_path) as f:
            span = json.loads(f.readline())

        duration = int(span['metadata']['http.duration_ms'])
        self.assertGreaterEqual(duration, 0)


if __name__ == '__main__':
    unittest.main()
