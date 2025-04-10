-- supabase/migrations/0001_add_user_profiles.sql

-- Create user_profiles table
create table public.user_profiles (
    id uuid references auth.users on delete cascade not null primary key,
    updated_at timestamp with time zone,
    username text unique,
    balance numeric not null default 1000.00, -- Start users with some funds

    constraint username_length check (char_length(username) >= 3)
);

-- Add RLS policies for user_profiles
alter table public.user_profiles enable row level security;

create policy "Profiles are viewable by everyone." on public.user_profiles
    for select using (true);

create policy "Users can insert their own profile." on public.user_profiles
    for insert with check (auth.uid() = id);

create policy "Users can update own profile." on public.user_profiles
    for update using (auth.uid() = id) with check (auth.uid() = id);

-- Function to create a profile for a new user
create or replace function public.handle_new_user()
returns trigger
language plpgsql
security definer set search_path = public
as $$
begin
  insert into public.user_profiles (id, username)
  values (new.id, new.raw_user_meta_data->>'username'); -- Assumes username is passed in metadata on signup
  return new;
end;
$$;

-- Trigger to call handle_new_user on new user creation
create trigger on_auth_user_created
  after insert on auth.users
  for each row execute procedure public.handle_new_user();

-- Function to update `updated_at` timestamp (can reuse from previous migration if desired)
-- Create if it doesn't exist, or ensure it works for user_profiles
create or replace function public.handle_profile_update()
returns trigger as $$
begin
  new.updated_at = timezone('utc'::text, now());
  return new;
end;
$$ language plpgsql security definer;

-- Trigger to automatically update `updated_at` on profile change
create trigger on_profile_update
  before update on public.user_profiles
  for each row execute procedure public.handle_profile_update();

-- (Optional) Add function to update balance safely (prevents direct updates)
-- create or replace function public.add_balance(user_id_input uuid, amount_to_add numeric)
-- returns numeric
-- language plpgsql
-- as $$
-- declare
--   new_balance numeric;
-- begin
--   update public.user_profiles
--   set balance = balance + amount_to_add
--   where id = user_id_input
--   returning balance into new_balance;
--   return new_balance;
-- end;
-- $$; 