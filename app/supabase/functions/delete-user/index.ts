import { createClient } from 'https://esm.sh/@supabase/supabase-js@2'

Deno.serve(async (req) => {
  // Handle CORS preflight requests
  if (req.method === 'OPTIONS') {
    return new Response('ok', {
      headers: {
        'Access-Control-Allow-Origin': '*',
        'Access-Control-Allow-Methods': 'POST, OPTIONS',
        'Access-Control-Allow-Headers': 'authorization, x-client-info, apikey, content-type',
      },
    })
  }

  // Get JWT from Authorization header
  const authHeader = req.headers.get('Authorization')
  if (!authHeader) {
    return new Response(
      JSON.stringify({ error: 'Missing authorization header' }),
      { status: 401, headers: { 'Content-Type': 'application/json' } }
    )
  }

  const supabaseUrl = Deno.env.get('SUPABASE_URL')!
  const supabaseAnonKey = Deno.env.get('SUPABASE_ANON_KEY')!
  const serviceRoleKey = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY')!

  // Create client with user's JWT to get their ID
  const userClient = createClient(supabaseUrl, supabaseAnonKey, {
    global: { headers: { Authorization: authHeader } }
  })

  const { data: { user }, error: userError } = await userClient.auth.getUser()
  if (userError || !user) {
    return new Response(
      JSON.stringify({ error: 'Invalid token' }),
      { status: 401, headers: { 'Content-Type': 'application/json' } }
    )
  }

  console.log(`Deleting user: ${user.id}`)

  // Create admin client with service role for deletion
  const adminClient = createClient(supabaseUrl, serviceRoleKey)

  try {
    // Delete user data from tables
    // Note: Order matters if there are no ON DELETE CASCADE constraints
    // Adjust table names as needed for your schema

    // Delete preferences
    const { error: prefError } = await adminClient
      .from('preferences')
      .delete()
      .eq('user_id', user.id)
    if (prefError) console.log('Preferences delete error (may not exist):', prefError.message)

    // Delete rooms (if user-owned)
    const { error: roomsError } = await adminClient
      .from('rooms')
      .delete()
      .eq('user_id', user.id)
    if (roomsError) console.log('Rooms delete error (may not exist):', roomsError.message)

    // Delete hubs (if user-owned)
    const { error: hubsError } = await adminClient
      .from('hubs')
      .delete()
      .eq('user_id', user.id)
    if (hubsError) console.log('Hubs delete error (may not exist):', hubsError.message)

    // Delete homes (cascade should handle related records if configured)
    const { error: homesError } = await adminClient
      .from('homes')
      .delete()
      .eq('user_id', user.id)
    if (homesError) console.log('Homes delete error (may not exist):', homesError.message)

    // Delete the auth user
    const { error: deleteError } = await adminClient.auth.admin.deleteUser(user.id)
    if (deleteError) {
      console.error('Auth user delete error:', deleteError.message)
      return new Response(
        JSON.stringify({ error: deleteError.message }),
        { status: 500, headers: { 'Content-Type': 'application/json' } }
      )
    }

    console.log(`Successfully deleted user: ${user.id}`)
    return new Response(
      JSON.stringify({ success: true }),
      { status: 200, headers: { 'Content-Type': 'application/json' } }
    )
  } catch (error) {
    console.error('Delete user error:', error)
    return new Response(
      JSON.stringify({ error: error.message || 'Unknown error' }),
      { status: 500, headers: { 'Content-Type': 'application/json' } }
    )
  }
})
