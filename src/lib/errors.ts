/**
 * Text for an error from a Tauri command or a thrown value. Tauri rejects with the Rust `String`
 * (a JS string) and, for structured errors, a `{ message }` object.
 */
export function errorMessage(error: unknown): string {
  if (typeof error === 'string') {
    return error
  }
  if (error instanceof Error) {
    return error.message
  }
  if (
    typeof error === 'object' &&
    error !== null &&
    'message' in error &&
    typeof error.message === 'string'
  ) {
    return error.message
  }
  return 'Something went wrong.'
}
