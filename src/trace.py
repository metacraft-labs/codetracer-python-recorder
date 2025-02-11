import os
import sys
import pprint
import json
from typing import NewType, List, Any, Dict
from dataclasses import dataclass

CallKey = NewType('CallKey', int)
StepId = NewType('StepId', int)
PathId = NewType('PathId', int)

@dataclass
class Call:
    key: CallKey
    path_id: PathId
    line: int
    name: str
    args: List['ArgRecord']
    return_value: 'Value'
    step_id: StepId
    depth: int
    parent_key: CallKey


@dataclass
class Step:
    step_id: StepId
    path_id: PathId
    line: int
    call_key: CallKey

@dataclass
class ArgRecord:
    name: str
    value: 'Value'

class Value:
    kind: str
    ti: int

    def __init__(self, kind: str, ti: int, **kwargs) -> None:
        self.kind = kind
        self.ti = ti
        for k, v in kwargs.items():
            setattr(self, k, v)

@dataclass
class Type:
    kind: int
    lang_type: str

INT = 7 # TODO
ERROR = 24 # TODO
NONE = 30 # TODO

INT_TYPE = Type(INT, 'int')
NO_TYPE = Type(NONE, '<no type>')

NO_KEY = CallKey(-1)

class TraceRecord:
    steps: List[Step]
    calls: List[Call]
    types: List[Type]
    events: List[Any]
    variables: List[List[Value]]
    flow: List[Any]
    paths: List[str]
    
    stack: List[CallKey]
    step_stack: List[StepId]
    path_map: Dict[str, PathId]
    current_call_key: CallKey

    def __init__(self) -> None:
        self.paths = []
        self.path_map = {}
        top_level = Call(CallKey(0), self.path_id(''), 1, '<toplevel>', [], NIL_VALUE, StepId(0), 0, NO_KEY)

        self.steps = []
        self.calls = [top_level]
        self.types = []
        self.events = []
        self.variables = []
        self.flow = []
        # self.paths = []

        self.stack = [top_level.key]
        self.step_stack = [top_level.step_id]
        # self.path_map = {}
        self.current_call_key = CallKey(top_level.key + 1)

    def register_step(self, path: str, line_arg: Any):
        try:
            line = int(line_arg)
        except ValueError as e:
             # TODO: sometimes "<module>": why?
            line = 1
        step = Step(self.next_step_id(), self.path_id(path), line, self.current_call_key)
        self.steps.append(step)
        self.step_stack[-1] = step.step_id
        if step.step_id == StepId(0):
            self.calls[self.stack[0]].path_id = self.path_id(path)
            self.calls[self.stack[0]].line = line

    def register_call(self, path: str, line_arg: int, name: str):
        try:
            line = int(line_arg)
        except ValueError as e:
             # TODO: sometimes "<module>": why?
            line = 1
        self.current_call_key = CallKey(self.current_call_key + 1)
        self.register_step(path, line)
        step_id = self.last_step_id()
        call = Call(
            self.current_call_key,
            self.path_id(path),
            line,
            name,
            [],
            NIL_VALUE,
            step_id,
            len(self.stack),
            self.stack[-1])
        self.calls.append(call)
        self.stack.append(call.key)
        self.step_stack.append(step_id)

    def register_return(self, path: str, line: int, arg: Any):
        # -> 
          # [0, 1]
          # [0, 5]
          # last in calls
        # <- 
          # [0]
          # [0]
          # return value for last
         
        return_value = self.load_value(arg)
        self.register_step(path, line)
        self.calls[-1].return_value = return_value
        self.stack.pop()
        self.step_stack.pop()

    def load_value(self, value: Any) -> Value:
        if isinstance(value, int):
            return self.int_value(value)
        else:
            return NOT_SUPPORTED_VALUE

    def int_value(self, i: int) -> Value:
        return Value('Int', ti=INT_TYPE_INDEX, i=i)

    def next_step_id(self) -> StepId:
        return StepId(len(self.steps))

    def last_step_id(self) -> StepId:
        return StepId(len(self.steps) - 1)

    def path_id(self, path: str) -> PathId:
        if path in self.path_map:
            return self.path_map[path]
        else:
            self.paths.append(path)
            path_id = PathId(len(self.paths) - 1)
            self.path_map[path] = path_id
            return path_id


INT_TYPE_INDEX = 0
NO_TYPE_INDEX = 1
NIL_VALUE = Value('None', ti=NO_TYPE_INDEX)
NOT_SUPPORTED_VALUE = Value('Error', ti=NO_TYPE_INDEX, msg='<not supported>')

TRACE = TraceRecord()
TRACE.types.append(INT_TYPE)
TRACE.types.append(NO_TYPE)



def trace_func(frame, event: str, arg):
    if event == 'call':
        if not frame.f_code.co_filename.startswith('<frozen '):
            TRACE.register_call(frame.f_code.co_filename, frame.f_code.co_name, frame.f_lineno)
        return trace_in_func

def trace_in_func(frame, event: str, arg):
    # print(dir(frame.f_code))
    if not frame.f_code.co_filename.startswith('<frozen '):
        if event == 'line':
            TRACE.register_step(frame.f_code.co_filename, frame.f_lineno)
        elif event == 'call':
            print('CALL')
        elif event == 'return':
            TRACE.register_return(frame.f_code.co_filename, frame.f_lineno, arg)
    return trace_in_func

sys.settrace(trace_func)

import calc

sys.settrace(None)



trace_values = {
    'program': sys.argv[0],
    'args': sys.argv[1:],
    'workdir': os.getcwd(),
    'steps': TRACE.steps,
    'calls': TRACE.calls,
    'variables': TRACE.variables,
    'types': TRACE.types,
    'events': TRACE.events,
    'paths': TRACE.paths
}
with open('trace.json', 'w') as file:
    file.write(json.dumps(trace_values, default=vars, indent=4))
#pprint.pp(TRACE.steps)
#pprint.pp(TRACE.calls)
#pprint.pp(TRACE.types)
#pprint.pp(TRACE.events)
