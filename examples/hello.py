def f(a, b):
    if a>0:
        return f(a-1, b+1)
    else:
        return b

print(f(5,1))

