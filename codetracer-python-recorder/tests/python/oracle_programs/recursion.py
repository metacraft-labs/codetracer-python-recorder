def factorial(n):
    if n <= 1:
        return 1
    rest = factorial(n - 1)
    return n * rest


result = factorial(5)
print(result)
