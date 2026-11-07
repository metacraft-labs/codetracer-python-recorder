def describe(name, count, active):
    label = name + ":" + str(count)
    if active:
        label = label + "!"
    flags = [count, count * 2]
    return label


def collect(limit):
    items = []
    i = 0
    while i < limit:
        if i % 2 == 0:
            items.append(i)
        i += 1
    return items


first = describe("alpha", 3, True)
second = describe("beta", 0, False)
evens = collect(6)
nothing = None
print(first)
print(second)
