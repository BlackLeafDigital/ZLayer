# TargetPlatform

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Arch** | [**ArchKind**](ArchKind.md) |  | 
**Os** | [**OsKind**](OsKind.md) |  | 
**OsVersion** | Pointer to **NullableString** | Optional OS version constraint — primarily for Windows multi-platform images, where &#x60;platform.os.version&#x60; in the OCI index distinguishes build families (e.g. &#x60;10.0.26100.*&#x60; for Server 2025 / Win11 24H2, &#x60;10.0.20348.*&#x60; for Server 2022). When set on a Windows target the registry platform resolver prefers manifest entries whose &#x60;os.version&#x60; matches this value exactly or shares a &#x60;major.minor.build&#x60; prefix. Unused on Linux/macOS platforms. | [optional] 

## Methods

### NewTargetPlatform

`func NewTargetPlatform(arch ArchKind, os OsKind, ) *TargetPlatform`

NewTargetPlatform instantiates a new TargetPlatform object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTargetPlatformWithDefaults

`func NewTargetPlatformWithDefaults() *TargetPlatform`

NewTargetPlatformWithDefaults instantiates a new TargetPlatform object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetArch

`func (o *TargetPlatform) GetArch() ArchKind`

GetArch returns the Arch field if non-nil, zero value otherwise.

### GetArchOk

`func (o *TargetPlatform) GetArchOk() (*ArchKind, bool)`

GetArchOk returns a tuple with the Arch field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetArch

`func (o *TargetPlatform) SetArch(v ArchKind)`

SetArch sets Arch field to given value.


### GetOs

`func (o *TargetPlatform) GetOs() OsKind`

GetOs returns the Os field if non-nil, zero value otherwise.

### GetOsOk

`func (o *TargetPlatform) GetOsOk() (*OsKind, bool)`

GetOsOk returns a tuple with the Os field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOs

`func (o *TargetPlatform) SetOs(v OsKind)`

SetOs sets Os field to given value.


### GetOsVersion

`func (o *TargetPlatform) GetOsVersion() string`

GetOsVersion returns the OsVersion field if non-nil, zero value otherwise.

### GetOsVersionOk

`func (o *TargetPlatform) GetOsVersionOk() (*string, bool)`

GetOsVersionOk returns a tuple with the OsVersion field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOsVersion

`func (o *TargetPlatform) SetOsVersion(v string)`

SetOsVersion sets OsVersion field to given value.

### HasOsVersion

`func (o *TargetPlatform) HasOsVersion() bool`

HasOsVersion returns a boolean if a field has been set.

### SetOsVersionNil

`func (o *TargetPlatform) SetOsVersionNil(b bool)`

 SetOsVersionNil sets the value for OsVersion to be an explicit nil

### UnsetOsVersion
`func (o *TargetPlatform) UnsetOsVersion()`

UnsetOsVersion ensures that no value is present for OsVersion, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


