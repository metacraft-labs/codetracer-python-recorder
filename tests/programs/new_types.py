def demo():
    class MyObj:
        def __init__(self):
            self.x = 10
            self.msg = "hi"

    fl = 1.5
    tpl = (1, 2, 3)
    bb = b'xy'
    cmplx = 1 + 2j
    obj = MyObj()
    return fl, tpl, bb, cmplx, obj

demo()
