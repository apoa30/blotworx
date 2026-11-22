//! Original vector4 userdata ABI. String formatting, metatable introspection
//! and special-value differential validation remain incomplete.
use mlua::{AnyUserData, MetaMethod, UserData, UserDataMethods, Value};

#[derive(Clone, Copy, Debug)]
pub struct Vector4(pub [f32; 4]);

pub fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    // REL2793b8 sets the argument stack to four, then uses the optional
    // lua_tonumber helper with +0 defaults and rounds each lane with frsp.
    lua.globals().set(
        "vector4",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let mut lanes = [0.0f32; 4];
            for (lane, value) in lanes.iter_mut().zip(args.into_iter()) {
                *lane = lua.coerce_number(value)?.unwrap_or(0.0) as f32;
            }
            Ok(Vector4(lanes))
        })?,
    )
}

impl Vector4 {
    pub(super) fn length3(self) -> mlua::Result<f32> {
        let squared = self.dot3(self);
        if squared == 0.0 {
            return Ok(0.0);
        }
        // REL1fad0 and REL27a084 share this frsqrte/frsp/refinement path.
        // Special-value payload behavior needs a separate differential capture.
        let inverse =
            super::native_hk_transform::inverse_root(squared).map_err(mlua::Error::external)?;
        Ok(squared * inverse)
    }
    pub(super) fn length2(self) -> mlua::Result<f32> {
        let squared = self.0[0].mul_add(self.0[0], self.0[2] * self.0[2]);
        if squared == 0.0 {
            return Ok(0.0);
        }
        if !squared.is_finite() || squared < 0.0 {
            return Err(mlua::Error::external(
                "Original vector4 length2 special-value behavior is not implemented",
            ));
        }
        // DOL800071f0 retains the double estimate until each single operation;
        // unlike length3, it uses 0.5*r*(3-r*r*s), not an additive refinement.
        let estimate = super::pose_blend::estimate_double(squared);
        let square = super::pose_blend::square_estimate(estimate);
        let half = (estimate * 0.5) as f32;
        let correction = -square.mul_add(squared, -3.0);
        let inverse = correction * half;
        Ok(squared * if inverse >= 0.0 { inverse } else { squared })
    }
    pub(super) fn normalized3(self) -> mlua::Result<Self> {
        let length = self.length3()?;
        Ok(if length > 0.0001f32 {
            let scale = 1.0f32 / length;
            Self(self.0.map(|v| v * scale))
        } else {
            self
        })
    }
    // REL279c4c: Z product fused into X product, then add Y product.
    fn dot3(self, b: Self) -> f32 {
        self.0[2].mul_add(b.0[2], self.0[0] * b.0[0]) + self.0[1] * b.0[1]
    }
    // REL279d1c paired merges store Y again in W; do not zero that lane.
    fn cross3(self, b: Self) -> Self {
        let x = self.0[1].mul_add(b.0[2], -(b.0[1] * self.0[2]));
        let y = -self.0[0].mul_add(b.0[2], -(b.0[0] * self.0[2]));
        let z = -self.0[1].mul_add(b.0[0], -(b.0[1] * self.0[0]));
        Self([x, y, z, y])
    }
}

impl UserData for Vector4 {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_function(
            MetaMethod::ToString,
            |_, _: AnyUserData| -> mlua::Result<String> {
                Err(mlua::Error::external(
                    "Original vector4 string formatting is not implemented",
                ))
            },
        );
        // Full userdata, not an ordinary Lua table. REL279490 converts the
        // selector with luaL_checklstring, including numeric selectors.
        methods.add_meta_function(
            MetaMethod::Index,
            |lua, (data, key): (AnyUserData, Value)| {
                let text = lua.coerce_string(key.clone())?.ok_or_else(|| {
                    mlua::Error::external("vector4 selector must be a string or number")
                })?;
                match read_selector(text.as_bytes().as_ref()) {
                    Read::Lane(lane) => Ok(Value::Number(data.borrow::<Vector4>()?.0[lane] as f64)),
                    Read::UnchangedStackTop => Ok(key),
                    Read::Length3 => Ok(Value::Number(data.borrow::<Vector4>()?.length3()? as f64)),
                    Read::Metatable => data.metatable()?.get(text.to_str()?.as_ref()),
                }
            },
        );
        methods.add_meta_function(
            MetaMethod::NewIndex,
            |lua, (data, key, value): (AnyUserData, Value, Value)| {
                let text = lua.coerce_string(key)?.ok_or_else(|| {
                    mlua::Error::external("vector4 selector must be a string or number")
                })?;
                // REL279690 uses lua_tonumber, not luaL_checknumber, then frsp.
                let value = lua.coerce_number(value)?.unwrap_or(0.0) as f32;
                if let Some(lane) = write_selector(text.as_bytes().as_ref()) {
                    data.borrow_mut::<Vector4>()?.0[lane] = value;
                }
                Ok(())
            },
        );
        // REL279884/279994: paired add/subtract include W and allocate new
        // userdata. REL279aa4 flips each sign bit, including signed zeros.
        methods.add_meta_function(MetaMethod::Add, |_, (a, b): (AnyUserData, AnyUserData)| {
            let a = a.borrow::<Vector4>()?;
            let b = b.borrow::<Vector4>()?;
            Ok(Vector4(std::array::from_fn(|i| a.0[i] + b.0[i])))
        });
        methods.add_meta_function(MetaMethod::Sub, |_, (a, b): (AnyUserData, AnyUserData)| {
            let a = a.borrow::<Vector4>()?;
            let b = b.borrow::<Vector4>()?;
            Ok(Vector4(std::array::from_fn(|i| a.0[i] - b.0[i])))
        });
        methods.add_meta_function(MetaMethod::Unm, |_, a: AnyUserData| {
            Ok(Vector4(
                a.borrow::<Vector4>()?
                    .0
                    .map(|v| f32::from_bits(v.to_bits() ^ 0x8000_0000)),
            ))
        });
        // REL279b64 requires a vector on the left; the last argument is
        // lua_tonumber (optional nil -> zero) rounded to f32 before multiplying.
        methods.add_meta_function(MetaMethod::Mul, |lua, (a, scale): (AnyUserData, Value)| {
            let scale = lua.coerce_number(scale)?.unwrap_or(0.0) as f32;
            Ok(Vector4(a.borrow::<Vector4>()?.0.map(|v| v * scale)))
        });
        methods.add_meta_function(
            MetaMethod::Concat,
            |_, (a, b): (AnyUserData, AnyUserData)| {
                Ok(a.borrow::<Vector4>()?.dot3(*b.borrow::<Vector4>()?))
            },
        );
        methods.add_meta_function(MetaMethod::Pow, |_, (a, b): (AnyUserData, AnyUserData)| {
            Ok(a.borrow::<Vector4>()?.cross3(*b.borrow::<Vector4>()?))
        });
        methods.add_method("length3", |_, value, _: mlua::MultiValue| value.length3());
        methods.add_method("length2", |_, value, _: mlua::MultiValue| value.length2());
        methods.add_method("getNormalized3", |_, value, _: mlua::MultiValue| {
            value.normalized3()
        });
        methods.add_function("normalize3", |_, args: mlua::MultiValue| {
            let Some(Value::UserData(data)) = args.front() else {
                return Err(mlua::Error::external(
                    "normalize3 requires vector4 userdata",
                ));
            };
            let mut value = data.borrow_mut::<Vector4>()?;
            *value = value.normalized3()?;
            // REL279ef4 returns one without pushing: preserve existing stack top.
            Ok(args.back().cloned().unwrap_or(Value::Nil))
        });
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    Lane(usize),
    Length3,
    Metatable,
    UnchangedStackTop,
}
fn c_string(key: &[u8]) -> &[u8] {
    &key[..key.iter().position(|&b| b == 0).unwrap_or(key.len())]
}
pub fn read_selector(key: &[u8]) -> Read {
    let key = c_string(key);
    if key.len() != 1 {
        return Read::Metatable;
    };
    match key[0] {
        b'1' | b'r' | b'x' => Read::Lane(0),
        b'2' | b'g' | b'y' => Read::Lane(1),
        b'3' | b'b' | b'z' => Read::Lane(2),
        b'4' | b'a' | b'w' => Read::Lane(3),
        b'l' => Read::Length3,
        _ => Read::UnchangedStackTop,
    }
}
pub fn write_selector(key: &[u8]) -> Option<usize> {
    match key.first().copied()? {
        b'1' | b'r' | b'x' => Some(0),
        b'2' | b'g' | b'y' => Some(1),
        b'3' | b'b' | b'z' => Some(2),
        b'4' | b'a' | b'w' => Some(3),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constructor_matches_original_coercion_and_argument_stack() -> mlua::Result<()> {
        let lua = mlua::Lua::new();
        install(&lua)?;
        lua.load(
            r#"
            local a = vector4('1.5', false, nil, -2, 100)
            assert(type(a) == 'userdata' and a.r == 1.5 and a.g == 0 and a.b == 0 and a.w == -2)
            local b = vector4(16777217)
            assert(b.r == 16777216 and b.g == 0 and b.b == 0 and b.w == 0)
            local z = vector4(); assert(z.l == 0)
            assert(not pcall(tostring, a))
        "#,
        )
        .exec()
    }
    #[test]
    fn vector_lengths_and_normalization_preserve_original_storage() -> mlua::Result<()> {
        let lua = mlua::Lua::new();
        lua.globals().set("v", Vector4([0.0, 3.0, 4.0, 10.0]))?;
        lua.globals()
            .set("tiny", Vector4([0.00001, 0.0, 0.0, 7.0]))?;
        lua.globals().set("zero", Vector4([0.0; 4]))?;
        lua.load(
            r#"
            assert(math.abs(v.l - 5) < 0.00001)
            assert(v:length3() == v.l and math.abs(v:length2() - 4) < 0.00001)
            local n = v:getNormalized3()
            assert(n ~= v and v.w == 10 and math.abs(n.w - 2) < 0.00001)
            assert(v:normalize3() == v and math.abs(v.w - 2) < 0.00001)
            assert(v:normalize3('top') == 'top')
            tiny:normalize3(); assert(tiny.w == 7)
            assert(zero:length2() == 0 and zero:length3() == 0)
        "#,
        )
        .exec()?;
        Ok(())
    }
    #[test]
    fn original_vector_operator_overloads_preserve_order_and_w() -> mlua::Result<()> {
        let lua = mlua::Lua::new();
        lua.globals().set("a", Vector4([1.0, 2.0, 3.0, 99.0]))?;
        lua.globals().set("b", Vector4([4.0, 5.0, 6.0, 77.0]))?;
        lua.load(
            r#"
            assert((a .. b) == 32)
            cross = a ^ b
            assert(cross.r == -3 and cross.g == 6 and cross.b == -3 and cross.w == 6)
            local scaled = a * '2'
            assert(scaled.r == 2 and scaled.w == 198)
            assert((a * nil).w == 0 and (a * {}).r == 0)
            assert(not pcall(function() return 2 * a end))
        "#,
        )
        .exec()?;
        Ok(())
    }
    #[test]
    fn userdata_field_access_and_arithmetic_follow_original_lanes() -> mlua::Result<()> {
        let lua = mlua::Lua::new();
        lua.globals().set("a", Vector4([1.0, 2.0, 3.0, 4.0]))?;
        lua.globals().set("b", Vector4([5.0, 6.0, 7.0, 8.0]))?;
        lua.load(
            r#"
            assert(type(a) == 'userdata' and a[1] == 1 and a.y == 2)
            assert(a.x == 'x' and a['?'] == '?')
            a.xyz = '9'; assert(a.r == 9)
            a.g = 'not a number'; assert(a.g == 0)
            c = a + b; d = c - b; n = -a
            assert(c.r == 14 and c.w == 12 and d.r == 9)
            assert(a.w == 4 and n.w == -4)
            assert(a:length3() == a.l)
        "#,
        )
        .exec()?;
        let n: AnyUserData = lua.globals().get("n")?;
        assert_eq!(n.borrow::<Vector4>()?.0[1].to_bits(), (-0.0f32).to_bits());
        Ok(())
    }
    #[test]
    fn original_vector_selectors_preserve_read_write_asymmetry() {
        for (lane, keys) in [b"1r".as_slice(), b"2gy", b"3bz", b"4aw"]
            .into_iter()
            .enumerate()
        {
            for &key in keys {
                assert_eq!(read_selector(&[key]), Read::Lane(lane));
                assert_eq!(write_selector(&[key]), Some(lane));
            }
        }
        assert_eq!(read_selector(b"x"), Read::UnchangedStackTop);
        assert_eq!(write_selector(b"x"), Some(0));
        assert_eq!(read_selector(b"xyz"), Read::Metatable);
        assert_eq!(write_selector(b"xyz"), Some(0));
        assert_eq!(read_selector(b"l"), Read::Length3);
        assert_eq!(read_selector(b"length3"), Read::Metatable);
        assert_eq!(read_selector(b"y\0junk"), Read::Lane(1));
        assert_eq!(read_selector(b"?"), Read::UnchangedStackTop);
        assert_eq!(read_selector(b""), Read::Metatable);
        assert_eq!(write_selector(b""), None);
    }
}
