#[derive(Default)]
pub(super) struct DynamicVertexBuffer {
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
}

impl DynamicVertexBuffer {
    pub(super) fn upload<T: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        vertices: &[T],
    ) -> Option<wgpu::Buffer> {
        let bytes = bytemuck::cast_slice(vertices);
        if bytes.is_empty() {
            return None;
        }
        let required = bytes.len() as u64;
        if required > self.capacity {
            self.capacity = required.next_power_of_two().max(256);
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: self.capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let buffer = self.buffer.as_ref()?;
        queue.write_buffer(buffer, 0, bytes);
        Some(buffer.clone())
    }
}
