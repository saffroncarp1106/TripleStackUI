import tkinter as tk
import os
import zipfile
import shutil
from tkinter import ttk, filedialog, messagebox
from pathlib import Path
from PIL import Image, ImageTk
from PIL import ImageEnhance
from image_utils import stack_overlay, make_pattern
from processor import copy_and_process
from resources import resource_path, MissingAssetError


class App(ttk.Frame):
    def __init__(self, root):
        super().__init__(root, padding=10)
        root.title("Triple Stack Skin Maker")
        root.geometry("700x480")
        self.skin_path = None
        self.orig_img = self.pre_img = self.num_img = None
        self.copy_var = tk.BooleanVar(value=True)
        self.ovr_var  = tk.BooleanVar(value=True)
        self.res_var  = tk.StringVar(value="160")
        self.modify_followpoint = tk.BooleanVar(value=False)
        self.use_hitcircle_overlay = tk.BooleanVar(value=True)
        self.overlay_on_top = tk.BooleanVar(value=False)
        self.last_skin_folder = None
        self.animation_id = None
        self._build_ui()
        self.pack(fill='both', expand=True)
        self._start_animation()


    def _build_ui(self):
        style = ttk.Style()
        style.theme_use('clam')
        style.configure('.', background='#2e2e2e', foreground='#fff')

        self.title_label = ttk.Label(self, text="Triple Stack Skin Maker",
                  font=('Segoe UI',16,'bold'))
        self.title_label.pack(pady=(0,6))
        ttk.Label(self, text="Back up first! Deletes @2x, stacks defaults & hitcircle, updates skin.ini.",
                  wraplength=500, justify='center').pack(pady=(0,8))

        self.opt_btn = ttk.Button(self, text="Options ▼", command=self._toggle_opts)
        self.opt_btn.pack(pady=5)

        # Modern options frame with dark styling
        self.opts = tk.Frame(self, bg='#1a1a1a', relief='solid', bd=1)
        row_frame = tk.Frame(self.opts, bg='#1a1a1a')
        row_frame.pack(pady=10, padx=15, fill='x')

        self.opts.pack(fill='x', padx=20, pady=6, after=self.opt_btn)
        self.opt_btn.config(text="Options ▲")

        # Hide options by default
        self.opts.pack_forget()
        self.opt_btn.config(text="Options ▼")

        # First row - Copy and Force Resolution
        cb1 = tk.Checkbutton(row_frame, text="Copy & Process", variable=self.copy_var,
                             bg='#1a1a1a', fg='#fff', selectcolor='#6b4da8', activebackground='#1a1a1a',
                             activeforeground='#ff66cc', font=('Segoe UI', 9))
        cb1.pack(side='left', padx=10, pady=5)

        cb2 = tk.Checkbutton(row_frame, text="Force Overlay Res", variable=self.ovr_var,
                             bg='#1a1a1a', fg='#fff', selectcolor='#6b4da8', activebackground='#1a1a1a',
                             activeforeground='#ff66cc', font=('Segoe UI', 9))
        cb2.pack(side='left', padx=10, pady=5)

        tk.Label(row_frame, text="Offset:", bg='#1a1a1a', fg='#aaa', font=('Segoe UI', 9)).pack(side='left', padx=(10,5))

        self.slider = tk.Scale(row_frame, from_=100, to=160, orient='horizontal',
                               command=self._slider_changed, bg='#2e2e2e', fg='#ff66cc',
                               highlightthickness=0, troughcolor='#383838', length=120)
        self.slider.pack(side='left', padx=(0,10))

        self.slider_val = tk.Label(row_frame, text='Native', bg='#1a1a1a', fg='#ff66cc', font=('Segoe UI', 9, 'bold'))
        self.slider_val.pack(side='left')

        self.slider.set(160)

        self.ovr_var.trace_add('write', lambda *a: self.update_preview())

        # Second row - Hitcircle options
        hitcircle_frame = tk.Frame(self.opts, bg='#1a1a1a')
        hitcircle_frame.pack(pady=5, padx=15, fill='x')

        cb3 = tk.Checkbutton(hitcircle_frame, text="Use Rafis hitcircle", variable=self.use_hitcircle_overlay,
                             command=self.update_preview, bg='#1a1a1a', fg='#fff', selectcolor='#6b4da8',
                             activebackground='#1a1a1a', activeforeground='#ff66cc', font=('Segoe UI', 9))
        cb3.pack(anchor='w', padx=10, pady=3)

        cb4 = tk.Checkbutton(hitcircle_frame, text="Use existing + Rafis circle", variable=self.overlay_on_top,
                             command=self.update_preview, bg='#1a1a1a', fg='#fff', selectcolor='#6b4da8',
                             activebackground='#1a1a1a', activeforeground='#ff66cc', font=('Segoe UI', 9))
        cb4.pack(anchor='w', padx=10, pady=3)

        cb5 = tk.Checkbutton(hitcircle_frame, text="Brighter Follow Point", variable=self.modify_followpoint,
                             bg='#1a1a1a', fg='#fff', selectcolor='#6b4da8', activebackground='#1a1a1a',
                             activeforeground='#ff66cc', font=('Segoe UI', 9))
        cb5.pack(anchor='w', padx=10, pady=3)

        bar = ttk.Frame(self); bar.pack(pady=(8,12))
        pat = make_pattern(120,40,'#ff66cc','#e055b3',10)
        for txt, cmd in [("Select Skin", self.select_skin),
                         ("Confirm",     self.confirm)]:
            btn = tk.Button(bar, text=txt, image=pat, compound='center',
                            bd=0, command=cmd, font=('Segoe UI', 10, 'bold'),
                            fg='#fff', activeforeground='#fff')
            btn.image = pat
            btn.pack(side='left', padx=8)
            if txt=="Confirm":
                self.confirm_btn = btn
                btn.config(state='disabled')

        self.canvas = tk.Canvas(self, bg='#2e2e2e',
                                highlightthickness=2, highlightbackground='#ff66cc')
        self.canvas.pack(fill='both', expand=True)
        self.canvas.bind('<Configure>', lambda e: self._draw_canvas())


    def _slider_changed(self, val):
        val = int(float(val))
        self.res_var.set(str(val))
        if self.ovr_var.get():
            self.slider_val.config(text=str(val))
        else:
            if self.skin_path:
                orig_f = self.skin_path / "hitcircle.png"
                if orig_f.exists():
                    o = Image.open(orig_f)
                    self.slider_val.config(text=f"Native ({o.width}x{o.height})")
                else:
                    self.slider_val.config(text="Native")
            else:
                self.slider_val.config(text="Native")
        self.update_preview()


    def _toggle_opts(self):
        if self.opts.winfo_ismapped():
            self.opts.pack_forget()
            self.opt_btn.config(text="Options ▼")
        else:
            self.opts.pack(fill='x', padx=20, pady=6, after=self.opt_btn)
            self.opt_btn.config(text="Options ▲")


    def _draw_canvas(self):
        self.canvas.delete('all')
        w, h = self.canvas.winfo_width(), self.canvas.winfo_height()

        for y in range(0, h, 20):
            for x in range(0, w, 20):
                self.canvas.create_polygon(
                    x, y+20, x+10, y, x+20, y+20,
                    fill='#444', outline='#2e2e2e'
                )

        imgs = []
        if self.orig_img:
            imgs.append(("Base", self.orig_img))
        if self.pre_img:
            imgs.append(("Overlayed", self.pre_img))
        if self.num_img:
            imgs.append(("Numbered", self.num_img))

        if not imgs:
            return

        spacing = w / (len(imgs) + 1)
        cy = h / 2

        for i, (label, img) in enumerate(imgs):
            cx = spacing * (i + 1)
            self.canvas.create_image(cx, cy, image=img, anchor='center')
            label_y = cy - (img.height()/2) - 8
            self.canvas.create_text(cx, label_y, text=label, anchor='center', fill='#fff')

    def _start_animation(self):
        """Start smooth animations"""
        self._animate_pulse(0)
        self._animate_canvas(0)

    def _animate_pulse(self, frame):
        """Create a subtle pulsing effect on the title"""
        if not hasattr(self, 'title_label'):
            return

        import math
        # Create a smooth pulse effect
        intensity = 0.7 + 0.3 * (0.5 + 0.5 * math.sin(frame * 0.05))
        color = f"#{int(255 * intensity):02x}{int(102 * intensity):02x}{int(204 * intensity):02x}"
        self.title_label.config(foreground=color)

        self.animation_id = self.after(50, lambda: self._animate_pulse(frame + 1))

    def _animate_canvas(self, frame):
        """Animate triangle patterns with smooth waves"""
        if not self.winfo_exists():
            return

        self.canvas.delete('all')
        w, h = self.canvas.winfo_width(), self.canvas.winfo_height()

        import math

        # Draw animated triangles with wave effect
        for y in range(0, h, 20):
            for x in range(0, w, 20):
                # Calculate wave offset based on position and time
                wave = math.sin((x * 0.02 + frame * 0.04)) * 2

                # Determine color - grey base with purple highlights
                wave_intensity = (math.sin(frame * 0.05 + (x + y) * 0.01) + 1) / 2

                if wave_intensity > 0.6:
                    # Purple highlight
                    color = '#6b4da8'
                else:
                    # Grey base
                    color = '#444'

                try:
                    self.canvas.create_polygon(
                        x, y + 20 + wave, x + 10, y + wave, x + 20, y + 20 + wave,
                        fill=color, outline='#2e2e2e'
                    )
                except:
                    pass

        imgs = []
        if self.orig_img:
            imgs.append(("Base", self.orig_img))
        if self.pre_img:
            imgs.append(("Overlayed", self.pre_img))
        if self.num_img:
            imgs.append(("Numbered", self.num_img))

        if imgs:
            spacing = w / (len(imgs) + 1)
            cy = h / 2

            for i, (label, img) in enumerate(imgs):
                cx = spacing * (i + 1)
                self.canvas.create_image(cx, cy, image=img, anchor='center')
                label_y = cy - (img.height()/2) - 8
                self.canvas.create_text(cx, label_y, text=label, anchor='center', fill='#fff')

        self.after(30, lambda: self._animate_canvas(frame + 1))

    def select_skin(self):
        d = filedialog.askdirectory(title="Select osu! Skin Folder", initialdir=self.last_skin_folder)
        if not d: return
        self.skin_path = Path(d)
        self.last_skin_folder = d
        self.confirm_btn.config(state='normal')
        self.update_preview()
        self._draw_canvas()


    def _warn_missing_bundled_hitcircle(self, path):
        """Surface a missing bundled asset once instead of silently doing nothing."""
        if getattr(self, '_warned_missing_hitcircle', False):
            return
        self._warned_missing_hitcircle = True
        messagebox.showerror(
            "Missing bundled asset",
            "hitcircle.png (the Rafis circle) is missing from this build, so the "
            "overlay cannot be applied.\n\nExpected at:\n"
            f"{path}\n\n"
            "Place hitcircle.png next to the executable, or rebuild with:\n"
            'pyinstaller --add-data "hitcircle.png;." ...'
        )

    def update_preview(self):
        p = self.skin_path
        if not p:
            return
        provided_f = resource_path("hitcircle.png")
        orig_f = p / "hitcircle.png"
        if self.overlay_on_top.get():
            if provided_f.exists() and orig_f.exists():
                base_img = Image.open(orig_f).convert('RGBA')
                overlay_img2 = Image.open(provided_f).convert('RGBA')
                if overlay_img2.size != base_img.size:
                    overlay_img2 = overlay_img2.resize(base_img.size, Image.LANCZOS)
                overlay_img = base_img.copy()
                overlay_img.alpha_composite(overlay_img2)
            elif provided_f.exists():
                overlay_img = Image.open(provided_f).convert('RGBA')
            elif orig_f.exists():
                overlay_img = Image.open(orig_f).convert('RGBA')
            else:
                return
        elif self.use_hitcircle_overlay.get():
            if provided_f.exists():
                overlay_img = Image.open(provided_f).convert('RGBA')
            else:
                self._warn_missing_bundled_hitcircle(provided_f)
                return
        else:
            if not orig_f.exists():
                return
            overlay_img = Image.open(orig_f).convert('RGBA')

        self.orig_img = ImageTk.PhotoImage(overlay_img)
        self.pre_img = ImageTk.PhotoImage(overlay_img)

        num_img_path = next(p.glob("default-[0-9]*.png"), None)
        if num_img_path and num_img_path.exists():
            default_num = Image.open(num_img_path).convert('RGBA')
            if self.ovr_var.get() and (r := self.res_var.get()) != "Native":
                out_size = (int(r), int(r))
            else:
                out_size = overlay_img.size
            canvas = Image.new('RGBA', out_size, (0, 0, 0, 0))
            base_pos = ((out_size[0] - default_num.width) // 2, (out_size[1] - default_num.height) // 2)
            canvas.paste(default_num, base_pos, default_num)
            overlay_resized = overlay_img.resize(out_size, Image.LANCZOS) if overlay_img.size != out_size else overlay_img
            stacked_num = stack_overlay(canvas, overlay_resized, out_size[0])
            self.num_img = ImageTk.PhotoImage(stacked_num)
        else:
            self.num_img = None

        self._draw_canvas()

    def _export_to_lazer(self, skin_path):
        default_name = f"{skin_path.name}@3xStack.osk"
        downloads_dir = str(Path.home() / "Downloads")
        save_path = filedialog.asksaveasfilename(defaultextension=".osk", initialfile=default_name, initialdir=downloads_dir, filetypes=[("OSK Files", "*.osk"), ("All Files", "*.*")])
        if save_path:
            try:
                with zipfile.ZipFile(save_path, 'w', zipfile.ZIP_DEFLATED) as zf:
                    for root, dirs, files in os.walk(skin_path):
                        for file in files:
                            file_path = Path(root) / file
                            arcname = file_path.relative_to(skin_path)
                            zf.write(file_path, arcname)
                messagebox.showinfo("Success", f"Skin exported to:\n{save_path}")
            except Exception as e:
                messagebox.showerror("Error", f"Export failed: {str(e)}")

    def confirm(self):
        res = self.res_var.get() if self.ovr_var.get() else None
        try:
            out = copy_and_process(self.skin_path, self.copy_var.get(), force_res=res, use_hitcircle_overlay=self.use_hitcircle_overlay.get(), overlay_on_top=self.overlay_on_top.get(), modify_followpoint=self.modify_followpoint.get())
        except MissingAssetError as e:
            messagebox.showerror("Missing asset", str(e))
            return
        except Exception as e:
            messagebox.showerror("Processing failed", f"{type(e).__name__}: {e}")
            return
        self._export_to_lazer(out)

if __name__ == "__main__":
    root = tk.Tk()
    app = App(root)
    app.mainloop()
