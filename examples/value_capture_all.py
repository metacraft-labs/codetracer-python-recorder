"""Single example exercising many variable-visibility scenarios for value capture.

Covers:
- Simple function locals/params
- Nested functions, nonlocal (closures)
- Globals read/write
- Class body scope and metaclass
- Lambdas and all comprehensions
- Generators and async coroutines
- try/except/else/finally and with
- Decorators and wrappers
- eval/exec dynamic names
- Imports at module and function scope
- Builtins usage
"""

from __future__ import annotations

#import asyncio
import math

# 1. Simple function: params and locals
def simple_function(x: int):
    a = 1
    b = a + x
    return a, b

# Globals
GLOBAL_VAL = 10
counter = 0
setting = "Hello"
CONSTANT = 42


# 8. Decorator and wrapper (captures free var `setting`)
def my_decorator(func):
    def wrapper(*args, **kwargs):
        # variables visible here: args, kwargs, setting, func (closed over)
        return func(*args, **kwargs)
    return wrapper


@my_decorator
def greet(name: str) -> str:
    message = f"Hi, {name}"
    return message




# 2. Nested functions and nonlocal
def outer_func(x: int):
    y = 1

    def inner_func(z: int):
        nonlocal y
        w = x + y + z
        y = w
        return w

    total = inner_func(5)
    return y, total


# 3. Globals
def global_test():
    local_copy = GLOBAL_VAL
    global counter
    counter += 1
    return local_copy, counter


# 4. Class scope and metaclass
class MetaCounter(type):
    count = 0

    def __init__(cls, name, bases, attrs):
        MetaCounter.count += 1
        super().__init__(name, bases, attrs)


class Sample(metaclass=MetaCounter):
    a = 10
    b = a + 5
    c = a + b + CONSTANT

    def method(self):
        return self.a + self.b


# 6. Generators and async coroutines
def counter_gen(n: int):
    total = 0
    for i in range(n):
        total += i
        yield total
    return total


# async def async_sum(data: list[int]) -> int:
#     total = 0
#     for x in data:
#         total += x
#         await asyncio.sleep(0)
#     return total


# async def agen(n: int):
#     for i in range(n):
#         yield i + 1


# 7. try/except/finally and with
def exception_and_with_demo(x: int):
    try:
        inv = 10 / x
    except ZeroDivisionError as e:
        error_msg = f"Error: {e}"
    else:
        inv += 1
    finally:
        final_flag = True

    with open(__file__, "r") as f:
        first_line = f.readline()
    return locals()


# 9. eval/exec
def eval_test():
    value = 10
    formula = "value * 2"
    result = eval(formula)
    return result


# 10. Imports and visibility
def import_test():
    import os
    constant = math.pi
    cwd = os.getcwd()
    return constant, cwd


# 11. Builtins
def builtins_test(seq):
    n = len(seq)
    m = max(seq)
    return n, m


def main() -> None:
    1
    res1 = simple_function(5)

    # 2
    res2 = outer_func(2)

    # 3
    before = counter
    _local_copy, _ctr = global_test()
    after = counter

    # 5. Lambdas and comprehensions
    factor = 2
    double = lambda y: y * factor  # noqa: E731
    squares = [n ** 2 for n in range(3)]
    scaled_set = {n * factor for n in range(3)}
    mapping = {n: n * factor for n in range(3)}
    gen_exp = (n * factor for n in range(3))
    result_list = list(gen_exp)

    # 6. Generators and async coroutines
    gen = counter_gen(3)
    gen_results = list(gen)
    #    coroutine_result = asyncio.run(async_sum([1, 2, 3]))

    # async def consume() -> int:
    #     acc = 0
    #     async for x in agen(3):
    #         acc += x
    #     return acc

    # async_acc = asyncio.run(consume())

    # 7. try/except/finally and with
    r1 = exception_and_with_demo(0)
    r2 = exception_and_with_demo(5)
    has_e = "error_msg" in r1
    has_inv = "inv" in r2
    has_final_flag = r1.get("final_flag", False) and r2.get("final_flag", False)

    # 8. Decorator and wrapper
    output = greet("World")

    # 9. eval/exec
    expr_code = "dynamic_var = 99"
    exec(expr_code, globals())
    dynamic_var = globals()["dynamic_var"]
    check = dynamic_var + 1
    out = eval_test()

    # 10. import visibility
    constant, cwd = import_test()

    # 11. builtins
    built_n, built_m = builtins_test([5, 3, 7])

    #Aggregate a compact, deterministic summary
    print(
        "ok",
        res1[0] + res1[1],                 # simple_function sum
        sum(res2),                         # outer_func sum
        after - before,                    # global counter increment
        MetaCounter.count,                 # metaclass incremented classes
        sum(squares), len(scaled_set), len(mapping), sum(result_list),
        sum(gen_results),                  # generator totals
        #coroutine_result, async_acc,       # async results
        has_e, has_inv, has_final_flag,    # exception/with signals
        len(output),                       # decorator + greet result length
        dynamic_var, check, out,           # eval/exec values
        f"{constant:.3f}",                 # math.pi to 3 decimals
        bool(len(cwd)),                    # cwd non-empty is True
        built_n, built_m,                  # builtins result
        double(7),                         # lambda capture
        Sample.c,                          # class body computed constant
    )


if __name__ == "__main__":
    main()
