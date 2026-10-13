"""Plain backdrop for recording the README demo, so nothing personal is in shot.

    py -3 assets/demo_backdrop.py X Y W H      (physical pixels; closes itself after 120 s)
"""
import ctypes
import sys
import tkinter as tk

ctypes.windll.shcore.SetProcessDpiAwareness(2)
x, y, w, h = (int(v) for v in sys.argv[1:5])
root = tk.Tk()
root.overrideredirect(True)
root.geometry(f"{w}x{h}+{x}+{y}")
root.configure(bg="#22272e")
root.after(120_000, root.destroy)
root.mainloop()
