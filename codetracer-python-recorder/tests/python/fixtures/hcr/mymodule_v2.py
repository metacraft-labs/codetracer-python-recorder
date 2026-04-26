def compute(n):
    return n * 3

def transform(value, n):
    return value - n

def aggregate(history):
    return max(history) if history else 0
