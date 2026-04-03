import * as React from "react"
import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'ghost' | 'outline' | 'error'
  size?: 'sm' | 'md' | 'lg' | 'icon'
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'primary', size = 'md', ...props }, ref) => {
    const variants = {
      primary: "bg-synod-accent text-black hover:bg-synod-accent/90 shadow-[0_0_20px_rgba(0,255,204,0.3)]",
      secondary: "bg-white/10 text-white hover:bg-white/20 border border-white/10",
      outline: "bg-transparent border border-synod-accent/50 text-synod-accent hover:bg-synod-accent/10",
      ghost: "bg-transparent hover:bg-white/10 text-gray-400 hover:text-white",
      error: "bg-synod-error/20 border border-synod-error/50 text-synod-error hover:bg-synod-error/30",
    }

    const sizes = {
      sm: "px-3 py-1.5 text-xs",
      md: "px-6 py-3 text-sm",
      lg: "px-8 py-4 text-base",
      icon: "p-2",
    }

    return (
      <button
        ref={ref}
        className={cn(
          "inline-flex items-center justify-center rounded-xl font-bold transition-all active:scale-95 disabled:opacity-50 disabled:pointer-events-none",
          variants[variant],
          sizes[size],
          className
        )}
        {...props}
      />
    )
  }
)
Button.displayName = "Button"

export { Button }
