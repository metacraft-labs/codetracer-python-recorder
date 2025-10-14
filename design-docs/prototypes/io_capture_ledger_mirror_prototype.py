#!/usr/bin/env python3
"""
Prototype that exercises the ledger + FD mirror algorithm with multiple Python
threads and direct ``os.write`` calls. The goal is to show that the ledger lets
the mirror drop bytes already captured by the proxies while still recording
native writes that bypass the proxies.

Run with:

    python design-docs/prototypes/io_capture_ledger_mirror_prototype.py
"""

from __future__ import annotations

import itertools
import os
import random
import sys
import threading
import time
from collections import deque
from dataclasses import dataclass
from typing import Deque, List, Sequence, Tuple


@dataclass
class LedgerEntry:
    seq: int
    data: bytes
    offset: int = 0

    @property
    def remaining(self) -> bytes:
        return self.data[self.offset :]

    def consume(self, size: int) -> bytes:
        chunk = self.data[self.offset : self.offset + size]
        self.offset += size
        return chunk

    def is_spent(self) -> bool:
        return self.offset >= len(self.data)


class Ledger:
    """Thread-safe FIFO ledger that stores proxy writes."""

    def __init__(self) -> None:
        self._entries: Deque[LedgerEntry] = deque()
        self._lock = threading.Lock()
        self._seq = itertools.count()

    def push(self, payload: bytes) -> int:
        seq = next(self._seq)
        entry = LedgerEntry(seq=seq, data=payload)
        with self._lock:
            self._entries.append(entry)
        return seq

    def subtract_from_chunk(self, chunk: bytes) -> Tuple[bytes, Sequence[Tuple[int, bytes]]]:
        """Remove ledger bytes from ``chunk`` while preserving native leftovers.

        Returns (leftover, matched_entries). ``leftover`` contains the bytes that
        did not match any ledger entry, in order. ``matched_entries`` keeps the
        ledger sequences that were consumed for debugging.
        """
        if not chunk:
            return b"", ()

        leftover = bytearray()
        matched: List[Tuple[int, bytes]] = []
        idx = 0
        view = memoryview(chunk)

        with self._lock:
            while idx < len(view):
                # If the ledger is empty we can append the rest of the chunk.
                if not self._entries:
                    leftover.extend(view[idx:])
                    break

                entry = self._entries[0]
                remaining = entry.remaining
                if not remaining:
                    self._entries.popleft()
                    continue

                # Skip native bytes that precede the next ledger entry.
                if view[idx] != remaining[0]:
                    leftover.append(view[idx])
                    idx += 1
                    continue

                # Full match fits inside the chunk.
                full_len = len(remaining)
                end_idx = idx + full_len
                if end_idx <= len(view) and view[idx:end_idx].tobytes() == remaining:
                    consumed = entry.consume(full_len)
                    matched.append((entry.seq, consumed))
                    idx = end_idx
                    if entry.is_spent():
                        self._entries.popleft()
                    continue

                # Partial match at the end of the chunk.
                tail = view[idx:].tobytes()
                prefix = remaining[: len(tail)]
                if tail == prefix:
                    consumed = entry.consume(len(tail))
                    matched.append((entry.seq, consumed))
                    idx = len(view)
                    if entry.is_spent():
                        self._entries.popleft()
                    break

                # The byte matches the ledger start but diverges immediately.
                # Treat it as native output.
                leftover.append(view[idx])
                idx += 1

        return (bytes(leftover), tuple(matched))

    def reset(self) -> None:
        with self._lock:
            self._entries.clear()

    def pending_bytes(self) -> int:
        with self._lock:
            return sum(len(entry.remaining) for entry in self._entries)


class ProxyStdout:
    """Minimal stdout proxy that records writes into the ledger."""

    def __init__(self, write_fd: int, ledger: Ledger, proxy_events: List[dict]) -> None:
        self._write_fd = write_fd
        self._ledger = ledger
        self._events = proxy_events
        self._lock = threading.RLock()
        self.encoding = "utf-8"
        self.errors = "strict"
        self.closed = False

    def write(self, text: str) -> int:
        if self.closed:
            raise ValueError("write to closed proxy")
        if not isinstance(text, str):
            text = str(text)
        if not text:
            return 0
        data = text.encode(self.encoding, self.errors)
        with self._lock:
            seq = self._ledger.push(data)
            self._events.append(
                {
                    "thread": threading.get_ident(),
                    "seq": seq,
                    "text": text,
                    "bytes": data,
                }
            )
            os.write(self._write_fd, data)
        return len(text)

    def writelines(self, lines: Sequence[str]) -> None:
        for line in lines:
            self.write(line)

    def flush(self) -> None:
        # Pipe writes are already flushed.
        return

    def fileno(self) -> int:
        return self._write_fd

    def isatty(self) -> bool:
        return False

    def close(self) -> None:
        if not self.closed:
            os.close(self._write_fd)
            self.closed = True


class FdMirror(threading.Thread):
    """Background thread that reads from the pipe and drops ledger matches."""

    def __init__(self, read_fd: int, ledger: Ledger, mirror_events: List[dict]) -> None:
        super().__init__(daemon=True)
        self._read_fd = read_fd
        self._ledger = ledger
        self._events = mirror_events
        self.matched_bytes = 0
        self.total_bytes = 0
        self._done = threading.Event()

    def run(self) -> None:
        try:
            while True:
                chunk = os.read(self._read_fd, 1024)
                if not chunk:
                    break
                self.total_bytes += len(chunk)
                leftover, matched = self._ledger.subtract_from_chunk(chunk)
                self.matched_bytes += sum(len(bytes_) for _, bytes_ in matched)
                if leftover:
                    payload = leftover.decode("utf-8", errors="replace")
                    self._events.append(
                        {
                            "thread": threading.get_ident(),
                            "payload": payload,
                            "raw_bytes": leftover,
                            "matched_sequences": [seq for seq, _ in matched],
                            "chunk_size": len(chunk),
                            "ledger_entries_consumed": len(matched),
                        }
                    )
        finally:
            self._done.set()

    def wait(self, timeout: float) -> bool:
        return self._done.wait(timeout)


def run_trial(trial_id: int, *, validate: bool = True) -> dict:
    orig_stdout = sys.stdout
    proxy_events: List[dict] = []
    mirror_events: List[dict] = []
    native_events: List[str] = []
    native_lock = threading.Lock()

    read_fd, write_fd = os.pipe()
    ledger = Ledger()
    proxy_stdout = ProxyStdout(write_fd=write_fd, ledger=ledger, proxy_events=proxy_events)
    sys.stdout = proxy_stdout

    mirror = FdMirror(read_fd=read_fd, ledger=ledger, mirror_events=mirror_events)
    mirror.start()

    random.seed(42 + trial_id)
    proxy_threads: List[threading.Thread] = []
    native_threads: List[threading.Thread] = []

    def proxy_worker(idx: int) -> None:
        for i in range(25):
            print(f"[proxy {idx}] message {i}")
            time.sleep(random.uniform(0.0, 0.003))

    def native_worker(idx: int) -> None:
        for i in range(15):
            payload = f"[native {idx}] chunk {i}\n"
            with native_lock:
                native_events.append(payload)
            os.write(write_fd, payload.encode("utf-8"))
            time.sleep(random.uniform(0.0, 0.004))

    for idx in range(4):
        t = threading.Thread(target=proxy_worker, args=(idx,), name=f"proxy-{idx}")
        proxy_threads.append(t)
        t.start()

    for idx in range(2):
        t = threading.Thread(target=native_worker, args=(idx,), name=f"native-{idx}")
        native_threads.append(t)
        t.start()

    for t in proxy_threads + native_threads:
        t.join()

    # Restore stdout before closing the pipe so that final prints go to the console.
    sys.stdout = orig_stdout

    proxy_stdout.close()
    mirror.wait(timeout=5.0)
    os.close(read_fd)
    mirror.join(timeout=0.1)

    native_payload = "".join(native_events)
    mirror_payload = "".join(event["payload"] for event in mirror_events)

    result = {
        "trial": trial_id,
        "proxy_events": len(proxy_events),
        "proxy_bytes": sum(len(event["bytes"]) for event in proxy_events),
        "native_events": len(native_events),
        "native_bytes": sum(len(payload.encode("utf-8")) for payload in native_events),
        "mirror_events": len(mirror_events),
        "mirror_bytes": sum(len(event["raw_bytes"]) for event in mirror_events),
        "ledger_pending": ledger.pending_bytes(),
        "mirror_matched_bytes": mirror.matched_bytes,
        "mirror_total_bytes": mirror.total_bytes,
    }

    if not validate:
        result["native_payload"] = native_payload
        result["mirror_payload"] = mirror_payload
        result["mirror_events_detail"] = mirror_events
        result["proxy_events_detail"] = proxy_events
        return result

    if native_payload != mirror_payload:
        raise AssertionError(
            "mirror payload does not match native writes",
            native_payload[:200],
            mirror_payload[:200],
        )

    if ledger.pending_bytes():
        raise AssertionError(f"ledger still holds {ledger.pending_bytes()} bytes")

    if not native_events:
        raise AssertionError("expected at least one native write")

    # Ensure proxy payloads never leak through the mirror events.
    for event in proxy_events:
        text = event["text"]
        if "[proxy" in text and text in mirror_payload:
            raise AssertionError("proxy payload leaked into mirror capture", event["text"])

    return result


def main() -> None:
    results = [run_trial(trial_id) for trial_id in range(5)]
    print("Ledger + FD mirror prototype results:")
    for entry in results:
        print(
            f"  trial {entry['trial']}: "
            f"proxy_bytes={entry['proxy_bytes']}, "
            f"native_bytes={entry['native_bytes']}, "
            f"mirror_bytes={entry['mirror_bytes']}, "
            f"matched_bytes={entry['mirror_matched_bytes']}, "
            f"total_bytes={entry['mirror_total_bytes']}"
        )
    print("All trials passed without ledger mismatches.")


if __name__ == "__main__":
    main()
