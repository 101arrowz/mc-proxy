
macro_rules! packets {
    (
        $(
            $packet:ident {
                $(
                    $field:ident : $type:ty
                ),* $(,)?
            }
        )*
    ) => {
        $(
            #[derive(Debug, Clone)]
            pub struct $packet {
                $(
                    pub $field: $type,
                )*
            }

            impl<'a, R: tokio::io::AsyncReadExt + Unpin + 'a> $crate::protocol::types::MCDecode<'a, R> for $packet {
                $crate::protocol::types::mcdecode_inner_impl!('a, R, src, version, {
                    Ok($packet {
                        $(
                            $field: <$type as $crate::protocol::types::MCDecode<'_, R>>::decode(src, version).await?,
                        )*
                    })
                });
            }

            impl<'a, W: tokio::io::AsyncWriteExt + Unpin + 'a> $crate::protocol::types::MCEncode<'a, W> for $packet {
                $crate::protocol::types::mcencode_inner_impl!('a, W, self, tgt, version, {
                    $(
                        $crate::protocol::types::MCEncode::encode(self.$field, tgt, version).await?;
                    )*
                    Ok(())
                });
            }
        )*
    };
}

pub(crate) use packets;