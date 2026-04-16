"use client"

import { useState } from "react"
import { useRouter } from "next/navigation"
import { cn } from "@/lib/utils"

export function LoginForm({
  className,
  ...props
}: React.ComponentProps<"form">) {
  const router = useRouter()
  const [email, setEmail] = useState("")
  const [password, setPassword] = useState("")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState("")

  const handleSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault()
    setLoading(true)
    setError("")

    try {
      const res = await fetch("/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "include",
        body: JSON.stringify({ email, password }),
      })

      if (!res.ok) {
        if (res.status === 401 || res.status === 403) {
          throw new Error("Invalid credentials")
        }

        let serverMessage = ""
        try {
          const data = (await res.json()) as { message?: string }
          serverMessage = data.message ?? ""
        } catch {
        }

        throw new Error(
          serverMessage || "Unable to sign in right now. Please try again shortly."
        )
      }

      router.push("/dashboard")
    } catch (err) {
      const message = err instanceof Error ? err.message : "Login failed"
      if (
        message.includes("Failed to fetch") ||
        message.includes("ECONNREFUSED") ||
        message.includes("fetch failed")
      ) {
        setError("Something went wrong")
      } else {
        setError(message)
      }
    } finally {
      setLoading(false)
    }
  }

  return (
    <>
      <div className="login-shell">
        <a href="#" className="page-logo">
          <div className="logo-icon">
            <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
              <polyline points="22 7 13.5 15.5 8.5 10.5 2 17" />
              <polyline points="16 7 22 7 22 13" />
            </svg>
          </div>
          Synod
        </a>

        <div className="login-wrapper">
          <div className="pre-header">
            <span className="pre-header-label">Secure Governance &amp; Treasury Coordination</span>
            <div className="gradient-hairline" />
          </div>

          <div className="header">
            <h1>Login to your account</h1>
            <p>Enter your email below to login to your account</p>
          </div>

          <form
            onSubmit={handleSubmit}
            className={cn("form", className)}
            {...props}
          >
            <div className="field">
              <label htmlFor="email">Email</label>
              <input
                id="email"
                type="email"
                placeholder="m@example.com"
                required
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                autoComplete="email"
              />
            </div>

            <div className="field">
              <div className="field-row">
                <label htmlFor="password">Password</label>
                <a href="#" className="forgot-link">Forgot your password?</a>
              </div>
              <input
                id="password"
                type="password"
                required
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete="current-password"
              />
            </div>

            {error ? <p className="error-message">{error}</p> : null}

            <button className="btn btn-primary" type="submit" disabled={loading}>
              {loading ? "Logging in..." : "Login"}
            </button>

            <div className="divider">
              <div className="divider-line" />
              <span className="divider-text">Or continue with</span>
              <div className="divider-line" />
            </div>

            <button className="btn btn-outline" type="button">
              <svg className="google-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                <path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z" fill="#4285F4" />
                <path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853" />
                <path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l3.66-2.84z" fill="#FBBC05" />
                <path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335" />
              </svg>
              Login with Google
            </button>
          </form>

          <p className="footer">
            Don&apos;t have an account? <a href="/signup">Sign up</a>
          </p>
        </div>
      </div>

      <style jsx global>{`
        @import url('https://fonts.googleapis.com/css2?family=Geist:wght@300;400;500;600&display=swap');

        *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

        :root {
          --background: #09090b;
          --foreground: #fafafa;
          --muted: #27272a;
          --muted-foreground: #71717a;
          --border: #27272a;
          --primary: #fafafa;
          --primary-foreground: #09090b;
          --radius: 0.5rem;
          --font: 'Geist', ui-sans-serif, system-ui, -apple-system, sans-serif;
        }

        html, body {
          height: 100%;
          background-color: var(--background);
          color: var(--foreground);
          font-family: var(--font);
          font-size: 14px;
          line-height: 1.5;
          -webkit-font-smoothing: antialiased;
        }

        body {
          display: flex;
          align-items: center;
          justify-content: center;
          min-height: 100vh;
          padding: 5rem 1rem 2rem;
          position: relative;
        }

        .login-shell {
          width: 100%;
          min-height: 100vh;
          display: flex;
          align-items: center;
          justify-content: center;
          padding: 5rem 1rem 2rem;
          position: relative;
          background-color: var(--background);
        }

        .page-logo {
          position: fixed;
          top: 1.5rem;
          left: 1.75rem;
          display: flex;
          align-items: center;
          gap: 0.5rem;
          text-decoration: none;
          color: var(--foreground);
          font-size: 0.875rem;
          font-weight: 600;
          letter-spacing: -0.01em;
          z-index: 10;
        }

        .logo-icon {
          width: 24px;
          height: 24px;
          background-color: var(--foreground);
          border-radius: 0.375rem;
          display: flex;
          align-items: center;
          justify-content: center;
          flex-shrink: 0;
        }

        .logo-icon svg {
          width: 13px;
          height: 13px;
          fill: none;
          stroke: var(--background);
          stroke-width: 2;
          stroke-linecap: round;
          stroke-linejoin: round;
        }

        .login-wrapper {
          width: 100%;
          max-width: 330px;
          display: flex;
          flex-direction: column;
          gap: 1.85rem;
        }

        .pre-header {
          display: flex;
          flex-direction: column;
          align-items: center;
          gap: 0.85rem;
        }

        .pre-header-label {
          font-size: 0.685rem;
          font-weight: 500;
          letter-spacing: 0.13em;
          text-transform: uppercase;
          color: var(--muted-foreground);
          text-align: center;
        }

        .gradient-hairline {
          width: 100%;
          height: 1px;
          border: none;
          background: linear-gradient(
            to right,
            transparent 0%,
            transparent 5%,
            rgba(140, 120, 255, 0.5) 28%,
            rgba(190, 170, 255, 0.95) 50%,
            rgba(140, 120, 255, 0.5) 72%,
            transparent 95%,
            transparent 100%
          );
          box-shadow:
            0 0 8px 0px rgba(170, 150, 255, 0.5),
            0 0 16px 2px rgba(160, 140, 255, 0.18);
        }

        .header {
          display: flex;
          flex-direction: column;
          align-items: center;
          gap: 0.4rem;
          text-align: center;
        }

        .header h1 {
          font-size: 1.625rem;
          font-weight: 600;
          letter-spacing: -0.025em;
          line-height: 1.2;
          color: var(--foreground);
        }

        .header p {
          font-size: 0.9rem;
          color: var(--muted-foreground);
          line-height: 1.45;
        }

        .form {
          display: flex;
          flex-direction: column;
          gap: 2rem;
        }

        .field {
          display: flex;
          flex-direction: column;
          gap: 0.4rem;
        }

        .field-row {
          display: flex;
          align-items: center;
          justify-content: space-between;
        }

        label {
          font-size: 0.9375rem;
          font-weight: 500;
          color: var(--foreground);
          line-height: 1;
        }

        .forgot-link {
          font-size: 0.75rem;
          color: var(--foreground);
          text-decoration: underline;
          text-underline-offset: 4px;
          text-decoration-color: rgba(250,250,250,0.22);
          transition: text-decoration-color 0.15s;
        }

        .forgot-link:hover {
          text-decoration-color: rgba(250,250,250,0.85);
        }

        input[type="email"],
        input[type="password"] {
          width: 100%;
          height: 2.6rem;
          padding: 0 0.75rem;
          border: 1px solid var(--border);
          border-radius: var(--radius);
          background-color: #18181b;
          font-size: 0.9375rem;
          font-family: var(--font);
          color: var(--foreground);
          outline: none;
          transition: box-shadow 0.15s, border-color 0.15s;
          -webkit-appearance: none;
        }

        input[type="email"]::placeholder,
        input[type="password"]::placeholder {
          color: var(--muted-foreground);
        }

        input[type="email"]:focus,
        input[type="password"]:focus {
          border-color: #52525b;
          box-shadow: 0 0 0 3px rgba(250, 250, 250, 0.06);
        }

        .btn {
          display: flex;
          align-items: center;
          justify-content: center;
          gap: 0.5rem;
          width: 100%;
          height: 2.6rem;
          padding: 0 1rem;
          border-radius: var(--radius);
          font-size: 0.9375rem;
          font-family: var(--font);
          cursor: pointer;
          border: none;
          transition: background-color 0.15s, opacity 0.15s;
          letter-spacing: -0.005em;
        }

        .btn:disabled {
          opacity: 0.7;
          cursor: default;
        }

        .btn-primary {
          background-color: var(--primary);
          color: var(--primary-foreground);
          font-weight: 500;
        }

        .btn-primary:hover { background-color: #e4e4e7; }

        .btn-outline {
          background-color: transparent;
          color: var(--foreground);
          border: 1px solid var(--border);
          font-weight: 500;
        }

        .btn-outline:hover { background-color: var(--muted); }

        .divider {
          display: flex;
          align-items: center;
          gap: 0.75rem;
        }

        .divider-line {
          flex: 1;
          height: 1px;
          background-color: var(--border);
        }

        .divider-text {
          font-size: 0.72rem;
          color: var(--muted-foreground);
          white-space: nowrap;
          text-transform: uppercase;
          letter-spacing: 0.05em;
        }

        .google-icon {
          width: 16px;
          height: 16px;
          flex-shrink: 0;
        }

        .footer {
          text-align: center;
          font-size: 0.875rem;
          color: var(--muted-foreground);
        }

        .footer a {
          color: var(--foreground);
          text-decoration: underline;
          text-underline-offset: 4px;
          font-weight: 500;
        }

        .footer a:hover { opacity: 0.8; }

        .error-message {
          margin-top: -0.75rem;
          color: #fca5a5;
          font-size: 0.75rem;
          line-height: 1.4;
          text-align: center;
        }
      `}</style>
    </>
  )
}
