def square(x):
    return x * x


def sum_of_squares(values):
    total = 0
    for v in values:
        total = total + square(v)
    return total


def report(values):
    s = sum_of_squares(values)
    big = s > 10
    return big


answer = report([1, 2, 3])
print(answer)
