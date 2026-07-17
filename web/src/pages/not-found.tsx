import { ArrowLeft } from 'lucide-react'
import { Link } from 'react-router-dom'

export function NotFoundPage() {
  return (
    <div className="not-found">
      <span>404</span>
      <h1>Page not found</h1>
      <Link className="button button--secondary" to="/"><ArrowLeft size={15} />Overview</Link>
    </div>
  )
}
