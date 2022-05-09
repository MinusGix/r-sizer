#[derive(Clone, Copy)]
#[repr(C)]
pub union FieldValue {
    // We'd have the other primitives too
    long: i64,
    int: i32,
    float: f32,
    double: f64,
    byte: i8,
    reference: u32,
    // A variant to represent that it does not have a good value
    invalid: (),
}
impl Default for FieldValue {
    fn default() -> FieldValue {
        FieldValue { invalid: () }
    }
}

use std::{
    alloc::{Layout, LayoutError},
    marker::PhantomData,
    ptr::NonNull,
};

/// Layout information for the layout
struct InstanceLayoutInfo<T> {
    /// The final layout
    layout: Layout,
    id_offset: usize,
    length_offset: usize,
    array_start_offset: usize,
    _marker: PhantomData<*const T>,
}
impl<T: Sized> InstanceLayoutInfo<T> {
    fn new(length: u16) -> Result<InstanceLayoutInfo<T>, LayoutError> {
        // Based on Layout::extend example in docs for getting fields for a #[repr(C)] structure

        let layout = BASE_LAYOUT?;

        // Add the id
        let (layout, id_offset) = layout.extend(ID_LAYOUT?)?;

        // Add the length
        let (layout, length_offset) = layout.extend(LENGTH_LAYOUT?)?;

        // UCG: The layout of a slice [T] of length N is the same as that of a [T; N] array.
        // and the docs for this says it is a record for [T; N]
        // thus we could treat this as a [T]
        let arrau_layout = Layout::array::<T>(usize::from(length)).unwrap();

        let (layout, array_start_offset) = layout.extend(arrau_layout)?;

        // TODO: Do we really need to do this? We aren't actually treating it as a C structure
        // currently, just as structure that provides accessors to pointer data.
        // We also aren't storing these directly sequentially in an array due their dynamic size
        let layout = layout.pad_to_align();

        Ok(InstanceLayoutInfo {
            layout,
            id_offset,
            length_offset,
            array_start_offset,
            _marker: PhantomData,
        })
    }
}

// Can't unwrap in a constant?
const BASE_LAYOUT: Result<Layout, LayoutError> = Layout::from_size_align(0, 1);

const ID_LAYOUT: Result<Layout, LayoutError> =
    Layout::from_size_align(std::mem::size_of::<u32>(), std::mem::size_of::<u32>());

const LENGTH_LAYOUT: Result<Layout, LayoutError> =
    Layout::from_size_align(std::mem::size_of::<u16>(), std::mem::align_of::<u16>());

// These functions should produce the same output as DstLayoutInfo would for their values

/// Compute the layout of the struct up to id, returning its offset and the layout
fn compute_id_layout_part() -> Result<(Layout, usize), LayoutError> {
    let layout = BASE_LAYOUT?;
    layout.extend(ID_LAYOUT?)
}

/// Compute the layout of the struct up to length, returning its offset and the layout
fn compute_length_layout_part() -> Result<(Layout, usize), LayoutError> {
    let layout = BASE_LAYOUT?;
    let (layout, _id_offset) = layout.extend(ID_LAYOUT?)?;
    layout.extend(LENGTH_LAYOUT?)
}

/// Compute the layout of the struct up to array, returning its offset and the layout
fn compute_array_layout_part<T>(length: u16) -> Result<(Layout, usize), LayoutError> {
    let (layout, _length_offset) = compute_length_layout_part()?;
    let arr_layout = Layout::array::<T>(usize::from(length)).unwrap();

    layout.extend(arr_layout)
}

/// We can't turn a pointer of bytes into a fat pointer
/// So we can't 'simply' return `*mut Dst` from `make_dst`
/// Thus, we store it in a structure as the opaque pointer, which
/// we must assume to be initialized.
struct OwnedInstanceRef<T> {
    ptr: NonNull<u8>,
    // TODO: Is this correct?
    _marker: PhantomData<*const T>,
}
impl<T: Sized> OwnedInstanceRef<T> {
    // Makes approximately
    // #[repr(C)]
    // struct Dst {
    //    id: u32,
    //    length: u16,
    //    data: [FieldValue],
    // }
    // Though we can't literally use the struct definition because we can't construct the fat
    // pointer for it. I think.
    // Thus we simply allocate the data in that manner, making an opaque wrapper structure
    // around a ptr.

    // Most of the asserts in this are optimized out
    pub fn new(
        id: u32,
        length: u16,
        default_elem_func: impl Fn(usize) -> T,
    ) -> Result<OwnedInstanceRef<T>, LayoutError> {
        assert!(
            isize::try_from(length).is_ok(),
            "Failed to fit length into isize"
        );
        assert!(
            usize::from(length)
                .checked_mul(std::mem::size_of::<T>())
                .is_some(),
            "Overflowed usize with the number of elements"
        );
        assert!(
            isize::try_from(usize::from(length) * std::mem::size_of::<T>()).is_ok(),
            "Overflowed isize with the number of elements"
        );

        let InstanceLayoutInfo {
            layout,
            id_offset,
            length_offset,
            array_start_offset,
            ..
        } = InstanceLayoutInfo::<T>::new(length)?;

        // Allocate the data

        let ptr: *mut u8 = unsafe { std::alloc::alloc(layout) };
        assert!(!ptr.is_null(), "Failed to allocate pointer");

        // Set id
        {
            // I imagine layout should return valid offsets anyway
            assert!(
                isize::try_from(id_offset).is_ok(),
                "Id offset overflows isize"
            );
            // Safety:
            // - offset was given by layout, and so should be in bounds of the allocation
            // - offset will not overflow an isize
            let id_ptr: *mut u8 = unsafe { ptr.add(id_offset) };
            let id_ptr: *mut u32 = id_ptr.cast::<u32>();
            unsafe { std::ptr::write(id_ptr, id) };
        };

        // Set length
        {
            assert!(
                isize::try_from(length_offset).is_ok(),
                "Length offset overflows isize"
            );

            // Safety:
            // - offset was given by layout, and so should be in bounds of the allocation
            // - offset will not overflow an isize
            let length_ptr: *mut u8 = unsafe { ptr.add(length_offset) };
            let length_ptr: *mut u16 = length_ptr.cast::<u16>();
            unsafe { std::ptr::write(length_ptr, length) };
        };

        // Set values
        {
            assert!(
                isize::try_from(array_start_offset).is_ok(),
                "Array data start offset overflows isize"
            );

            // Safety:
            // - offset was given by layout, and so should be in bounds of the allocation
            // - offset will not overflow an isize
            let arr_start_ptr: *mut u8 = unsafe { ptr.add(array_start_offset) };
            let arr_start_ptr: *mut T = arr_start_ptr.cast::<T>();

            assert!(
                isize::try_from(length).is_ok(),
                "Length would overflow isize"
            );
            for i in 0..length {
                // Safety:
                // - index should be valid since we told the layout to allocate an array of the
                // length
                // - index should also not overflow an isize, since length did not overflow an
                // isize
                let arr_element_ptr = unsafe { arr_start_ptr.add(usize::from(i)) };
                let value = default_elem_func(usize::from(i));
                unsafe {
                    std::ptr::write(arr_element_ptr, value);
                }
            }
        }

        // Shouldn't panic because we already checked that it is non-null
        let ptr = NonNull::new(ptr).unwrap();

        // Safety: We've initialized all the fields to valid values
        Ok(OwnedInstanceRef {
            ptr,
            _marker: PhantomData,
        })
    }

    pub fn id(&self) -> u32 {
        // Should not panic since we had to do the same logic to construct this instance
        // in the first place
        let (_id_layout, id_offset) = compute_id_layout_part().unwrap();
        // Safety: The construction of the structure should only have been done through
        // the `new` function which ensures this is a valid pointer and holds initialized
        // memory.
        let id_ptr: *const u8 = unsafe { self.ptr.as_ptr().add(id_offset) };
        let id_ptr: *const u32 = id_ptr.cast::<u32>();

        unsafe { std::ptr::read(id_ptr) }
    }

    pub fn length(&self) -> u16 {
        // Should not panic since we had to do the same logic to construct this instance
        // in the first place
        let (_length_layout, length_offset) = compute_length_layout_part().unwrap();
        // Safety: The construction of the structure should only have been done through
        // the `new` function which ensures this is a valid pointer and holds initialized
        // memory.

        let length_ptr: *const u8 = unsafe { self.ptr.as_ptr().add(length_offset) };
        let length_ptr: *const u16 = length_ptr.cast::<u16>();

        unsafe { std::ptr::read(length_ptr) }
    }

    pub fn as_slice(&self) -> &[T] {
        let length = self.length();

        // Should not panic since we had to do the same logic to construct this instance
        // in the first place
        let (_array_layout, array_start_offset) =
            compute_array_layout_part::<FieldValue>(length).unwrap();

        let array_start_ptr: *const u8 = unsafe { self.ptr.as_ptr().add(array_start_offset) };
        let array_start_ptr: *const T = array_start_ptr.cast::<T>();

        let length = usize::from(length);

        // Safety:
        // - Data is initialized for length reads
        // - Should be aligned due to layout
        // - The backing array won't be mutated because the pointer is only accessed through the
        // reference and so the borrow checker will stop it from calling mutating methods
        unsafe { std::slice::from_raw_parts(array_start_ptr, length) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        let length = self.length();

        // Should not panic since we had to do the same logic to construct this instance
        // in the first place
        let (_array_layout, array_start_offset) = compute_array_layout_part::<T>(length).unwrap();

        let array_start_ptr: *mut u8 = unsafe { self.ptr.as_ptr().add(array_start_offset) };
        let array_start_ptr: *mut T = array_start_ptr.cast::<T>();

        let length = usize::from(length);

        // Safety:
        // - Data is initialized for length reads
        // - Should be aligned due to layout
        // - The backing array won't be mutated because the pointer is only accessed through the
        // reference and so the borrow checker will stop it from calling other methods
        unsafe { std::slice::from_raw_parts_mut(array_start_ptr, length) }
    }

    pub fn get(&self, i: u16) -> Option<&T> {
        // We need the length to compute the proper layout and check validity
        let length = self.length();

        if i >= length {
            // Index was out of bounds
            return None;
        }

        // From here on out, the index is valid

        // Should not panic since we had to do the same logic to construct this instance
        // in the first place
        let (_array_layout, array_start_offset) = compute_array_layout_part::<T>(length).unwrap();

        let array_start_ptr: *const u8 = unsafe { self.ptr.as_ptr().add(array_start_offset) };
        let array_start_ptr: *const T = array_start_ptr.cast::<T>();

        // Safety: i must be valid since the constructor assured that all indices up to length
        // are valid, and we have asserted that `i < length`
        let array_element_ptr: *const T = unsafe { array_start_ptr.add(usize::from(i)) };

        // Safety: We are in a reference to the data, and so we can convert the ptr to a reference
        // to the data held
        let element = unsafe { &*array_element_ptr };
        Some(element)
    }

    pub fn get_mut(&mut self, i: u16) -> Option<&mut T> {
        // We need the length to compute the proper layout and check validity
        let length = self.length();

        if i >= length {
            // Index was out of bounds
            return None;
        }

        // From here on out, the index is valid

        // Should not panic since we had to do the same logic to construct this instance
        // in the first place
        let (_array_layout, array_start_offset) = compute_array_layout_part::<T>(length).unwrap();

        let array_start_ptr: *mut u8 = unsafe { self.ptr.as_ptr().add(array_start_offset) };
        let array_start_ptr: *mut T = array_start_ptr.cast::<T>();

        // Safety: i must be valid since the constructor assured that all indices up to length
        // are valid, and we have asserted that `i < length`
        let array_element_ptr: *mut T = unsafe { array_start_ptr.add(usize::from(i)) };

        // Safety: We are a mutable reference and so we are uniquely referenced and so we can return
        // a unique reference to the data inside us.
        let element = unsafe { &mut *array_element_ptr };
        Some(element)
    }
}
impl<T: Sized> Drop for OwnedInstanceRef<T> {
    fn drop(&mut self) {
        let length = self.length();
        let ptr = self.ptr.as_ptr();

        // Fill it with garbage so any UAFs are more likely to combust
        self.ptr = NonNull::dangling();

        let layout_info = InstanceLayoutInfo::<T>::new(length).unwrap();
        let layout = layout_info.layout;
        let array_start = layout_info.array_start_offset;

        // Drop the elements
        let array_ptr = unsafe { ptr.add(array_start) };
        let array_ptr = array_ptr.cast::<T>();
        for i in 0..length {
            let elem_ptr = unsafe { array_ptr.add(usize::from(i)) };
            let value = unsafe { std::ptr::read(elem_ptr) };
            drop(value);
        }

        // Safety: The pointer should be valid
        // and we can't drop twice.
        unsafe { std::alloc::dealloc(ptr, layout) };
    }
}

fn main() {
    // DstWrapper would be in a module so it couldn't be constructed as a struct literal
    // and could only go through new
    let mut val =
        OwnedInstanceRef::<FieldValue>::new(5, 4, |i| FieldValue { invalid: () }).unwrap();
    let id = val.id();
    println!("Id: {}", id);
    let length = val.length();
    println!("Length: {}", length);

    for i in 0..length {
        let value = val.get_mut(i).unwrap();
        value.int = i32::from(i);
        println!("Value at {} is {}", i, unsafe { value.int });
    }

    for i in 0..length {
        let value = val.get_mut(i).unwrap();
        unsafe { value.int *= 2 };
    }

    let data = val.as_slice();
    println!(
        "Data: {:?}",
        data.iter().map(|x| unsafe { x.int }).collect::<Vec<_>>()
    );
}
