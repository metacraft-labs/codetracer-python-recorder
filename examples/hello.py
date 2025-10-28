print("A")
import abcd
print("B")
def f(a, b):
    if a>0:
        out = f(a-1, b+1)
        return out
    else:
        return b

print(f(5,1))

print("BYE")
